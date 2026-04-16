#![cfg(not(target_arch = "wasm32"))]
//! Loopback integration test for the native [`WebRtcChannel`] impl.
//!
//! Wires two [`NativeWebRtcChannel`] instances via an in-memory signaling
//! shim that relays offer / answer / ICE candidates. No STUN / TURN
//! servers — libdatachannel gathers host candidates on 127.0.0.1 only,
//! which is sufficient for same-process loopback and requires no external
//! network.
//!
//! Acceptance criteria (per bead mxdx-awe.14 / storm):
//! - both sides reach `ChannelEvent::Open`
//! - a message sent on one side arrives on the other
//! - total test runtime < 2s on CI (enforced via tokio::time::timeout)
//!
//! Runs only on native targets — wasm cannot use libdatachannel.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use mxdx_p2p::channel::{ChannelEvent, IceCandidate, NativeWebRtcChannel, WebRtcChannel};

/// Total wall-clock budget for the loopback flow. Bead acceptance says
/// "under 2s" but ICE host-only negotiation typically completes in
/// ~100ms; 2s is ample headroom.
const TEST_BUDGET: Duration = Duration::from_secs(2);

/// Poll-interval when draining events between steps of the negotiation.
/// libdatachannel fires callbacks on its own thread; the mpsc receiver
/// simply hands us whatever is buffered.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Drain all currently-queued events from a channel's receiver without
/// blocking. Returns the collected events.
fn drain_events(ch: &mut NativeWebRtcChannel) -> Vec<ChannelEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = ch.events().try_recv() {
        out.push(ev);
    }
    out
}

/// Run the full loopback handshake and one round-trip message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_offer_answer_send() {
    tokio::time::timeout(TEST_BUDGET, async {
        let mut alice = NativeWebRtcChannel::new();
        let mut bob = NativeWebRtcChannel::new();

        // 1. Alice creates the offer. With no ICE servers, libdatachannel
        //    will only gather host candidates on localhost.
        let offer = alice
            .create_offer(&[])
            .await
            .expect("alice create_offer");

        // 2. Bob accepts the offer and produces the answer.
        let answer = bob
            .accept_offer(&[], offer)
            .await
            .expect("bob accept_offer");

        // 3. Alice accepts Bob's answer.
        alice.accept_answer(answer).await.expect("alice accept_answer");

        // 4. Relay ICE candidates in a background loop until both sides
        //    reach Open or the budget expires. libdatachannel gathers
        //    candidates asynchronously, so we must keep pumping.
        let mut alice_open = false;
        let mut bob_open = false;

        let start = std::time::Instant::now();
        while !(alice_open && bob_open) {
            if start.elapsed() > TEST_BUDGET {
                panic!(
                    "timed out waiting for Open; alice_open={alice_open} bob_open={bob_open}"
                );
            }

            // Drain Alice's events → forward to Bob / record Open.
            for ev in drain_events(&mut alice) {
                match ev {
                    ChannelEvent::LocalIce(c) => {
                        // Forward Alice's local candidate as Bob's remote.
                        let remote = IceCandidate {
                            candidate: c.candidate,
                            sdp_mid: c.sdp_mid,
                            sdp_mline_index: c.sdp_mline_index,
                        };
                        bob.add_ice_candidate(remote)
                            .await
                            .expect("bob add_ice_candidate (from alice)");
                    }
                    ChannelEvent::Open => alice_open = true,
                    ChannelEvent::Failure(msg) => panic!("alice failure: {msg}"),
                    ChannelEvent::Closed { reason } => {
                        panic!("alice closed unexpectedly: {reason}")
                    }
                    ChannelEvent::Message(_) => {
                        // No messages expected during handshake; ignore.
                    }
                }
            }

            // Drain Bob's events → forward to Alice / record Open.
            for ev in drain_events(&mut bob) {
                match ev {
                    ChannelEvent::LocalIce(c) => {
                        let remote = IceCandidate {
                            candidate: c.candidate,
                            sdp_mid: c.sdp_mid,
                            sdp_mline_index: c.sdp_mline_index,
                        };
                        alice
                            .add_ice_candidate(remote)
                            .await
                            .expect("alice add_ice_candidate (from bob)");
                    }
                    ChannelEvent::Open => bob_open = true,
                    ChannelEvent::Failure(msg) => panic!("bob failure: {msg}"),
                    ChannelEvent::Closed { reason } => {
                        panic!("bob closed unexpectedly: {reason}")
                    }
                    ChannelEvent::Message(_) => {}
                }
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // 5. Both sides are open — send a round-trip message. We use an
        //    opaque byte string to reinforce the "byte pipe" nature of
        //    the channel (payload is never inspected).
        let payload: &[u8] = b"encrypted-loopback-frame-\x00\x01\x02";
        alice.send(payload).await.expect("alice send");

        // 6. Wait for Bob to receive the Message event.
        let mut received: Option<bytes::Bytes> = None;
        let recv_deadline = std::time::Instant::now() + Duration::from_millis(500);
        while received.is_none() {
            if std::time::Instant::now() > recv_deadline {
                panic!("bob never received the loopback frame");
            }
            for ev in drain_events(&mut bob) {
                if let ChannelEvent::Message(b) = ev {
                    received = Some(b);
                    break;
                }
            }
            if received.is_none() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
        assert_eq!(
            received.as_deref(),
            Some(payload),
            "round-trip payload must match exactly"
        );

        // 7. Clean close on both sides.
        alice.close("loopback test done").await.expect("alice close");
        bob.close("loopback test done").await.expect("bob close");
    })
    .await
    .expect("loopback exceeded TEST_BUDGET");
}
