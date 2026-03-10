import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { fetchTurnCredentials, turnToIceServers } from '../../../packages/core/turn-credentials.js';

describe('TURN credentials', () => {
  it('parses homeserver TURN response into ICE servers', () => {
    const turnResponse = {
      username: '1443779631:@user:example.com',
      password: 'JlKfBy1QwLrO20385QyAtEyIv0=',
      uris: [
        'turn:turn.example.com:3478?transport=udp',
        'turn:turn.example.com:3478?transport=tcp',
        'turns:turn.example.com:5349?transport=tcp',
      ],
      ttl: 86400,
    };

    const iceServers = turnToIceServers(turnResponse);
    assert.equal(iceServers.length, 1);
    assert.deepEqual(iceServers[0].urls, turnResponse.uris);
    assert.equal(iceServers[0].username, turnResponse.username);
    assert.equal(iceServers[0].credential, turnResponse.password);
  });

  it('returns empty array for null/empty response', () => {
    assert.deepEqual(turnToIceServers(null), []);
    assert.deepEqual(turnToIceServers({}), []);
    assert.deepEqual(turnToIceServers({ uris: [] }), []);
  });

  it('fetchTurnCredentials calls correct endpoint', async () => {
    let calledUrl = null;
    const mockFetch = async (url, opts) => {
      calledUrl = url;
      return {
        ok: true,
        json: async () => ({
          username: 'user',
          password: 'pass',
          uris: ['turn:turn.example.com:3478'],
          ttl: 86400,
        }),
      };
    };

    const result = await fetchTurnCredentials('https://matrix.example.com', 'syt_token', mockFetch);
    assert.ok(calledUrl.includes('/_matrix/client/v3/voip/turnServer'));
    assert.equal(result.username, 'user');
  });

  it('returns null when homeserver has no TURN', async () => {
    const mockFetch = async () => ({ ok: false, status: 404 });
    const result = await fetchTurnCredentials('https://matrix.example.com', 'syt_token', mockFetch);
    assert.equal(result, null);
  });

  it('rejects non-https URLs', async () => {
    const mockFetch = async () => { throw new Error('should not be called'); };
    const result = await fetchTurnCredentials('ftp://evil.com', 'syt_token', mockFetch);
    assert.equal(result, null);
  });

  it('uses URL constructor for safe path building', async () => {
    let calledUrl = null;
    const mockFetch = async (url) => {
      calledUrl = url;
      return { ok: true, json: async () => ({ username: 'u', password: 'p', uris: ['turn:t:3478'], ttl: 86400 }) };
    };
    await fetchTurnCredentials('https://matrix.example.com/extra/path/', 'tok', mockFetch);
    assert.equal(calledUrl, 'https://matrix.example.com/_matrix/client/v3/voip/turnServer');
  });
});
