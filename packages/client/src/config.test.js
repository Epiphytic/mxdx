import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import * as TOML from 'smol-toml';
import { ClientConfig } from './config.js';

describe('ClientConfig', () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-client-config-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('creates config from CLI args', () => {
    const config = ClientConfig.fromArgs({
      username: 'liamhelmer',
      server: 'https://matrix.org',
      batchMs: '150',
    });
    assert.strictEqual(config.username, 'liamhelmer');
    assert.strictEqual(config.server, 'https://matrix.org');
    assert.strictEqual(config.batchMs, 150);
  });

  it('saves and loads TOML config in flat-key layout (T-3.3)', () => {
    const configPath = path.join(tmpDir, 'client.toml');
    const config = ClientConfig.fromArgs({
      username: 'liamhelmer',
      server: 'https://matrix.org',
    });
    config.save(configPath);
    assert.ok(fs.existsSync(configPath));

    // Must be flat top-level keys (no [client] wrapper)
    const raw = fs.readFileSync(configPath, 'utf8');
    assert.ok(!raw.includes('[client]'), 'saved file must not contain [client] section');
    const parsed = TOML.parse(raw);
    assert.strictEqual(typeof parsed.username, 'string', 'username must be top-level key');

    const loaded = ClientConfig.load(configPath);
    assert.strictEqual(loaded.username, 'liamhelmer');
    assert.deepStrictEqual(loaded.servers, ['https://matrix.org']);
  });

  it('returns null when config file does not exist', () => {
    const loaded = ClientConfig.load(path.join(tmpDir, 'nonexistent.toml'));
    assert.strictEqual(loaded, null);
  });

  it('migrates legacy [client]-wrapped config and preserves values (T-3.2)', () => {
    const configPath = path.join(tmpDir, 'client.toml');
    const legacy = `\n[client]\nusername = "liamhelmer"\nservers = ["https://matrix.org"]\nbatch_ms = 150\n`;
    fs.writeFileSync(configPath, legacy);

    const loaded = ClientConfig.load(configPath);
    assert.ok(loaded !== null);
    assert.strictEqual(loaded.username, 'liamhelmer');
    assert.deepStrictEqual(loaded.servers, ['https://matrix.org']);
    assert.strictEqual(loaded.batchMs, 150);

    const bak = fs.readFileSync(`${configPath}.legacy.bak`, 'utf8');
    assert.strictEqual(bak, legacy);

    const onDisk = fs.readFileSync(configPath, 'utf8');
    assert.ok(!onDisk.includes('[client]'), 'migrated file must not contain [client]');
  });

  it('tolerates unknown TOML keys without error (T-3.6)', () => {
    const configPath = path.join(tmpDir, 'client.toml');
    const content = `username = "test"\nfuture_field = "x"\n`;
    fs.writeFileSync(configPath, content);

    const loaded = ClientConfig.load(configPath);
    assert.ok(loaded !== null, 'should load without error despite unknown key');
    assert.strictEqual(loaded.username, 'test');
  });

  it('save preserves unrelated fields from existing file (T-3.4)', () => {
    const configPath = path.join(tmpDir, 'client.toml');
    // A Rust-written file might have fields that the npm client doesn't own
    const existing = `username = "liamhelmer"\nrust_only_field = "keep_me"\n`;
    fs.writeFileSync(configPath, existing);

    const config = new ClientConfig({ username: 'liamhelmer-updated' });
    config.save(configPath);

    const raw = fs.readFileSync(configPath, 'utf8');
    const parsed = TOML.parse(raw);
    assert.strictEqual(parsed.rust_only_field, 'keep_me', 'unowned fields must survive save()');
    assert.strictEqual(parsed.username, 'liamhelmer-updated');
  });
});
