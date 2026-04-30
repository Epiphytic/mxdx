import { NodeWebRTCChannel, P2PSignaling, generateSessionKey, createP2PCrypto, fetchTurnCredentials, turnToIceServers } from '@mxdx/core';
// Rust equivalent: crates/mxdx-worker/src/p2p/ — node-datachannel native addon, OS-bound

/**
 * Attempt to establish a P2P WebRTC connection for a room.
 * Launcher offers first; also listens for incoming client offers.
 * State tracking (rate limits, stale checks, settled flag) delegated to WASM SessionTransportManager.
 * Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager
 *
 * @param {object} opts
 * @param {object} opts.transport - P2PTransport instance
 * @param {object} opts.transportMgr - SessionTransportManager (WASM)
 * @param {string} opts.dmRoomId
 * @param {string} opts.signalingRoomId - exec room (established E2EE)
 * @param {object} opts.matrixClient - client with sendEvent / onRoomEvent / exportSession
 * @param {object} opts.config - { p2pTurnOnly }
 * @param {function} opts.log - logger
 */
export async function attemptP2PConnection({ transport, transportMgr, dmRoomId, signalingRoomId, matrixClient, config, log }) {
  if (!transportMgr.shouldAttemptP2P(dmRoomId)) { log.debug('P2P: rate limited', { room_id: dmRoomId }); return; }
  const attemptId = transportMgr.beginP2PAttempt(dmRoomId);
  log.info('Attempting P2P connection', { room_id: dmRoomId, attempt: attemptId });

  const session = JSON.parse(matrixClient.exportSession());
  const turnCreds = await fetchTurnCredentials(session.homeserver_url, session.access_token);
  const iceServers = turnCreds ? turnToIceServers(turnCreds) : [];
  if (turnCreds) log.info('P2P: TURN credentials fetched', { uris: turnCreds.uris?.length || 0 });
  else log.warn('P2P: no TURN credentials available');

  const turnOnly = config.p2pTurnOnly === true;
  const isStale = () => transportMgr.isAttemptStale(dmRoomId, attemptId);
  const settle = (channel, callId, role, p2pCrypto) => {
    if (transportMgr.isSettled(dmRoomId)) { channel.close(); return false; }
    transportMgr.markSettled(dmRoomId);
    if (p2pCrypto) transport.setP2PCrypto(p2pCrypto);
    transport.setDataChannel(channel);
    log.info('P2P data channel established', { room_id: dmRoomId, call_id: callId, role });
    return true;
  };

  const wireIceBatching = (channel, signaling, callId, partyId) => {
    const candidates = []; let t = null;
    channel.onIceCandidate((c) => { candidates.push(c); if (t) clearTimeout(t); t = setTimeout(async () => { const batch = candidates.splice(0); if (batch.length) await signaling.sendCandidates({ callId, partyId, candidates: batch }).catch(() => {}); }, 100); });
  };

  const pollCandidates = async (channel, callId, ownPartyId) => {
    try {
      const existing = JSON.parse(await matrixClient.findRoomEvents(signalingRoomId, 'm.call.candidates', 20));
      for (const evt of existing) { const c = evt.content || evt; if (c.call_id !== callId || c.party_id === ownPartyId) continue; for (const cand of (c.candidates || [])) channel.addIceCandidate(cand); }
    } catch { /* not critical */ }
    for (let i = 0; i < 30; i++) {
      if (transportMgr.isSettled(dmRoomId) && i > 5) return;
      const candJson = await matrixClient.onRoomEvent(signalingRoomId, 'm.call.candidates', 1);
      if (candJson == null) continue;
      try { const e = JSON.parse(candJson); const c = e.content || e; if (c.call_id !== callId || c.party_id === ownPartyId) continue; for (const cand of (c.candidates || [])) channel.addIceCandidate(cand); } catch { /* malformed */ }
    }
  };

  const makeSignaling = () => new P2PSignaling({ sendEvent: (rId, t, c) => matrixClient.sendEvent(rId, t, c), onRoomEvent: (rId, cb) => matrixClient.onRoomEvent(rId, cb) }, signalingRoomId, matrixClient.userId());
  const offererCallId = P2PSignaling.generateCallId();

  const offerPath = async () => {
    await new Promise((r) => setTimeout(r, 5000));
    if (transportMgr.isSettled(dmRoomId) || isStale()) return;
    const channel = new NodeWebRTCChannel({ iceServers, turnOnly });
    const signaling = makeSignaling(); const callId = offererCallId; const partyId = P2PSignaling.generatePartyId();
    wireIceBatching(channel, signaling, callId, partyId);
    const offerKey = await generateSessionKey(); const offerCrypto = await createP2PCrypto(offerKey);
    const offer = await channel.createOffer();
    await signaling.sendInvite({ callId, partyId, sdp: offer.sdp, lifetime: 30000, sessionKey: offerKey });
    let answerContent = null;
    try { const existing = JSON.parse(await matrixClient.findRoomEvents(signalingRoomId, 'm.call.answer', 10)); for (const evt of existing) { const c = evt.content || evt; if (c.call_id === callId) { answerContent = c; break; } } } catch { /* ok */ }
    const deadline = Date.now() + 30_000;
    while (!answerContent && Date.now() < deadline && !transportMgr.isSettled(dmRoomId) && !isStale()) {
      const aj = await matrixClient.onRoomEvent(signalingRoomId, 'm.call.answer', 5);
      if (aj == null) continue;
      const e = JSON.parse(aj); const c = e.content || e; if (c.call_id !== callId) continue;
      answerContent = c;
    }
    if (!answerContent || transportMgr.isSettled(dmRoomId)) { channel.close(); throw new Error('No P2P answer within timeout'); }
    await channel.acceptAnswer({ sdp: answerContent.answer.sdp, type: answerContent.answer.type });
    channel.onStateChange((state) => log.debug('P2P offerer ICE state', { state, call_id: callId }));
    pollCandidates(channel, callId, partyId).catch(() => {});
    await Promise.race([channel.waitForDataChannel(), new Promise((_, r) => setTimeout(() => r(new Error('Data channel timeout')), 30_000))]);
    settle(channel, callId, 'offerer', offerCrypto);
  };

  const answererPath = async () => {
    let inviteJson = null; const deadline = Date.now() + 35_000;
    while (Date.now() < deadline && !transportMgr.isSettled(dmRoomId) && !isStale()) {
      const json = await matrixClient.onRoomEvent(signalingRoomId, 'm.call.invite', 5);
      if (json == null) continue;
      const evt = JSON.parse(json); const c = evt.content || evt;
      if (c.call_id === offererCallId) continue;
      inviteJson = json; break;
    }
    if (transportMgr.isSettled(dmRoomId) || isStale() || !inviteJson) return;
    const inviteEvent = JSON.parse(inviteJson); const inviteContent = inviteEvent.content || inviteEvent;
    const callId = inviteContent.call_id;
    if (!callId || !inviteContent.offer?.sdp) return;
    let answererCrypto = null;
    if (inviteContent.mxdx_session_key) answererCrypto = await createP2PCrypto(inviteContent.mxdx_session_key);
    const channel = new NodeWebRTCChannel({ iceServers, turnOnly });
    const signaling = makeSignaling(); const partyId = P2PSignaling.generatePartyId();
    wireIceBatching(channel, signaling, callId, partyId);
    const answer = await channel.acceptOffer({ sdp: inviteContent.offer.sdp, type: 'offer' });
    await signaling.sendAnswer({ callId, partyId, sdp: answer.sdp });
    channel.onStateChange((state) => log.debug('P2P answerer ICE state', { state, call_id: callId }));
    pollCandidates(channel, callId, partyId).catch(() => {});
    await Promise.race([channel.waitForDataChannel(), new Promise((_, r) => setTimeout(() => r(new Error('Data channel timeout')), 30_000))]);
    settle(channel, callId, 'answerer', answererCrypto);
  };

  await Promise.any([offerPath().catch((e) => { throw e; }), answererPath().catch((e) => { throw e; })]).catch((err) => {
    const msg = err instanceof AggregateError ? err.errors.map(e => e.message).join('; ') : err.message;
    throw new Error(msg);
  });
}
