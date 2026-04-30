import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import * as TOML from 'smol-toml';
import { LauncherConfig } from './config.js';

describe('LauncherConfig', () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-config-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('creates config from CLI args', () => {
    const config = LauncherConfig.fromArgs({
      username: 'belthanior',
      servers: 'matrix.org,mxdx.dev',
      allowedCommands: 'echo,cat',
      telemetry: 'full',
      maxSessions: 10,
    });
    assert.strictEqual(config.username, 'belthanior');
    assert.deepStrictEqual(config.servers, ['matrix.org', 'mxdx.dev']);
    assert.deepStrictEqual(config.allowedCommands, ['echo', 'cat']);
  });

  it('saves and loads TOML config in flat-key layout', () => {
    const configPath = path.join(tmpDir, 'launcher.toml');
    const config = LauncherConfig.fromArgs({
      username: 'test-host',
      servers: 'matrix.org',
      allowedCommands: 'echo',
    });
    config.save(configPath);
    assert.ok(fs.existsSync(configPath));

    // Saved file must use flat top-level keys (no [launcher] wrapper)
    const raw = fs.readFileSync(configPath, 'utf8');
    assert.ok(!raw.includes('[launcher]'), 'saved file must not contain [launcher] section');
    const parsed = TOML.parse(raw);
    assert.strictEqual(typeof parsed.username, 'string', 'username must be a top-level key');

    const loaded = LauncherConfig.load(configPath);
    assert.strictEqual(loaded.username, 'test-host');
    assert.deepStrictEqual(loaded.servers, ['matrix.org']);
  });

  it('returns null when config file does not exist', () => {
    const loaded = LauncherConfig.load(path.join(tmpDir, 'nonexistent.toml'));
    assert.strictEqual(loaded, null);
  });

  it('loads flat-key config written by Rust mxdx-worker', () => {
    // Simulate a config file written by the Rust mxdx-worker (flat top-level keys)
    const configPath = path.join(tmpDir, 'worker.toml');
    const rustWritten = `username = "belthanior"\nallowed_commands = ["echo", "ls"]\nallowed_cwd = ["/tmp"]\nmax_sessions = 3\nauthorized_users = ["@alice:example.com"]\n`;
    fs.writeFileSync(configPath, rustWritten);

    const loaded = LauncherConfig.load(configPath);
    assert.ok(loaded !== null, 'should parse Rust-written flat config');
    assert.deepStrictEqual(loaded.allowedCommands, ['echo', 'ls']);
    assert.strictEqual(loaded.maxSessions, 3);
  });

  it('migrates legacy [launcher]-wrapped config and preserves values', () => {
    const configPath = path.join(tmpDir, 'worker.toml');
    const legacy = `\n[launcher]\nusername = "belthanior"\nallowed_commands = ["echo"]\nmax_sessions = 7\n`;
    fs.writeFileSync(configPath, legacy);

    const loaded = LauncherConfig.load(configPath);
    assert.ok(loaded !== null);
    assert.strictEqual(loaded.username, 'belthanior');
    assert.deepStrictEqual(loaded.allowedCommands, ['echo']);
    assert.strictEqual(loaded.maxSessions, 7);

    // .legacy.bak should exist with original content
    const bak = fs.readFileSync(`${configPath}.legacy.bak`, 'utf8');
    assert.strictEqual(bak, legacy);

    // File on disk should now be flat
    const onDisk = fs.readFileSync(configPath, 'utf8');
    assert.ok(!onDisk.includes('[launcher]'), 'migrated file must not contain [launcher]');
  });

  it('tolerates unknown TOML keys without error (T-3.6, ADR req 5)', () => {
    const configPath = path.join(tmpDir, 'worker.toml');
    // File contains an unrecognized future_field that neither runtime currently knows about
    const content = `max_sessions = 4\nallowed_commands = ["echo"]\nfuture_field = "x"\n`;
    fs.writeFileSync(configPath, content);

    // Must load without throwing
    const loaded = LauncherConfig.load(configPath);
    assert.ok(loaded !== null, 'should load without error despite unknown key');
    assert.strictEqual(loaded.maxSessions, 4);
    assert.deepStrictEqual(loaded.allowedCommands, ['echo']);
  });

  it('save preserves Rust-written unrelated fields (authorized_users)', () => {
    // Simulate a Rust-written file containing authorized_users (a field npm does not own)
    const configPath = path.join(tmpDir, 'worker.toml');
    const rustWritten = `username = "belthanior"\nauthorized_users = ["@alice:example.com", "@bob:example.com"]\nallowed_commands = ["cat"]\n`;
    fs.writeFileSync(configPath, rustWritten);

    // npm launcher updates its own fields via save()
    const config = new LauncherConfig({ username: 'belthanior-updated', allowedCommands: ['cat', 'ls'] });
    config.save(configPath);

    // authorized_users must survive the npm save round-trip unchanged
    const raw = fs.readFileSync(configPath, 'utf8');
    const parsed = TOML.parse(raw);
    assert.deepStrictEqual(
      parsed.authorized_users,
      ['@alice:example.com', '@bob:example.com'],
      'Rust-written authorized_users must survive npm config writer'
    );
    assert.strictEqual(parsed.username, 'belthanior-updated');
  });
});
