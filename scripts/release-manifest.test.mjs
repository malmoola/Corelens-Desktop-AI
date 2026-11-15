/**
 * The release manifest: the right files picked, the manifest the updater
 * reads shaped as it expects, the version's notes cut from the release
 * notes, and a signature's trusted comment read back.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { assetName } from './asset-names.mjs';
import { checksums, manifest, notesFor, pickArtifacts, trustedComment } from './release-manifest.mjs';

const NAMES = [
  'Corelens-Desktop-AI_2.9.0_x64-setup.exe',
  'Corelens-Desktop-AI_2.9.0_x64-setup.exe.sig',
  'SHA256SUMS.txt',
  'corelens-rust.cdx.json',
  'signing-root.crt',
  'signing.crt',
  'Corelens-Desktop-AI_2.9.0_universal.dmg',
  'Corelens-Desktop-AI.app.tar.gz',
  'Corelens-Desktop-AI.app.tar.gz.sig',
];

describe('pickArtifacts', () => {
  it('finds each installer, its signature and the extras worth attaching', () => {
    const picked = pickArtifacts(NAMES);
    expect(picked.windows).toBe('Corelens-Desktop-AI_2.9.0_x64-setup.exe');
    expect(picked.windowsSig).toBe('Corelens-Desktop-AI_2.9.0_x64-setup.exe.sig');
    expect(picked.mac).toBe('Corelens-Desktop-AI.app.tar.gz');
    expect(picked.macSig).toBe('Corelens-Desktop-AI.app.tar.gz.sig');
    expect(picked.dmg).toBe('Corelens-Desktop-AI_2.9.0_universal.dmg');
    // The Windows job's checksum file covers its installer only; the release
    // writes one for everything it publishes instead.
    expect(picked.extras).toEqual(['corelens-rust.cdx.json', 'signing-root.crt', 'signing.crt']);
  });

  it('refuses a release the app could not verify', () => {
    expect(() => pickArtifacts(NAMES.filter((n) => !n.endsWith('.sig')))).toThrow(/signature/);
    expect(() => pickArtifacts(NAMES.filter((n) => !n.endsWith('.dmg')))).toThrow(/disk image/);
    expect(() => pickArtifacts([...NAMES, 'Corelens-Desktop-AI_2.9.1_x64-setup.exe'])).toThrow(/Windows installer/);
  });

  // The bundler names files after the product, which has a space; GitHub
  // would upload them under another name than the manifest's URL.
  it('refuses a file name with a space, and the rename takes the space out', () => {
    const spaced = NAMES.map((n) => n.replace('Corelens-Desktop-AI', 'Corelens Desktop AI'));
    expect(() => pickArtifacts(spaced)).toThrow(/space/);
    expect(pickArtifacts(spaced.map(assetName)).windows).toBe('Corelens-Desktop-AI_2.9.0_x64-setup.exe');
    expect(assetName('Corelens Desktop AI.app.tar.gz.sig')).toBe('Corelens-Desktop-AI.app.tar.gz.sig');
  });
});

describe('manifest', () => {
  const picked = pickArtifacts(NAMES);
  const made = manifest({
    version: '2.9.0',
    repo: 'owner/name',
    picked,
    signatures: { windows: 'dW50cnVzdGVk', mac: 'bWFj' },
    notes: 'Fixes.\n',
    date: '2026-10-09T12:00:00Z',
  });

  it('points every platform at the tagged release on GitHub, with its signature', () => {
    expect(made.version).toBe('2.9.0');
    expect(made.pub_date).toBe('2026-10-09T12:00:00Z');
    expect(made.notes).toBe('Fixes.\n');
    expect(made.platforms['windows-x86_64']).toEqual({
      signature: 'dW50cnVzdGVk',
      url: 'https://github.com/owner/name/releases/download/v2.9.0/Corelens-Desktop-AI_2.9.0_x64-setup.exe',
    });
    // The universal archive serves both Mac targets.
    expect(made.platforms['darwin-aarch64']).toEqual({
      signature: 'bWFj',
      url: 'https://github.com/owner/name/releases/download/v2.9.0/Corelens-Desktop-AI.app.tar.gz',
    });
    expect(made.platforms['darwin-x86_64']).toEqual(made.platforms['darwin-aarch64']);
    expect(Object.keys(made.platforms)).toHaveLength(3);
  });

  it('refuses anything but a release version', () => {
    expect(() => manifest({ version: 'v2.9.0', repo: 'o/n', picked, signatures: { windows: 'a', mac: 'b' }, notes: '', date: '' })).toThrow(/not a release version/);
  });
});

describe('notesFor', () => {
  const NOTES = `# Release notes

Newest first.

## 2.9.1 — 2026-10-20

**Fixes**
- One.

## 2.9.0 — 2026-10-09

**Updates**
- The app can check GitHub.

## 2.8.0 — 2026-10-09

**Backups**
- Over SNMP.
`;

  it('cuts the version\'s own section, without its heading', () => {
    expect(notesFor(NOTES, '2.9.0')).toBe('**Updates**\n- The app can check GitHub.\n');
    expect(notesFor(NOTES, '2.9.1')).toBe('**Fixes**\n- One.\n');
    // The oldest section runs to the end of the file.
    expect(notesFor(NOTES, '2.8.0')).toBe('**Backups**\n- Over SNMP.\n');
  });

  it('says when the notes have no section for the version', () => {
    expect(() => notesFor(NOTES, '2.10.0')).toThrow(/no section for 2\.10\.0/);
    // 2.9.0 must not match 2.9.0's neighbour by prefix.
    expect(() => notesFor(NOTES.replace('## 2.9.0', '## 2.9.01'), '2.9.0')).toThrow();
  });
});

describe('trustedComment', () => {
  it('reads what a minisign signature says it signed', () => {
    const sig = Buffer.from(
      'untrusted comment: signature from tauri secret key\nRUSv…\ntrusted comment: timestamp:1791601794\tfile:sigtest.bin\n3YPk…\n',
    ).toString('base64');
    expect(trustedComment(sig)).toBe('timestamp:1791601794\tfile:sigtest.bin');
    expect(trustedComment(Buffer.from('nothing here').toString('base64'))).toBe('');
  });
});

describe('checksums', () => {
  it('lists every installer and archive a person can download, in the form sha256sum -c reads', () => {
    const files = [
      { name: 'a-setup.exe', bytes: Buffer.from('windows') },
      { name: 'a.dmg', bytes: Buffer.from('mac') },
      { name: 'a.app.tar.gz', bytes: Buffer.from('update') },
    ];
    const sha = (b) => createHash('sha256').update(b).digest('hex');
    expect(checksums(files)).toBe(files.map((f) => `${sha(f.bytes)}  ${f.name}`).join('\n') + '\n');
  });
});

describe('the release job', () => {
  it('attaches a SHA256SUMS.txt that covers the Windows installer, the disk image and the update archive', () => {
    const dir = mkdtempSync(join(tmpdir(), 'cv-release-'));
    const dl = join(dir, 'dl');
    mkdirSync(join(dl, 'windows'), { recursive: true });
    mkdirSync(join(dl, 'macos'), { recursive: true });
    const sig = Buffer.from('untrusted comment: x\nRUS\ntrusted comment: timestamp:1\tfile:x\nAA\n').toString('base64');
    writeFileSync(join(dl, 'windows', 'C_2.13.0_x64-setup.exe'), 'windows');
    writeFileSync(join(dl, 'windows', 'C_2.13.0_x64-setup.exe.sig'), sig);
    writeFileSync(join(dl, 'windows', 'SHA256SUMS.txt'), 'only the exe\n');
    writeFileSync(join(dl, 'macos', 'C_2.13.0_universal.dmg'), 'mac');
    writeFileSync(join(dl, 'macos', 'C.app.tar.gz'), 'update');
    writeFileSync(join(dl, 'macos', 'C.app.tar.gz.sig'), sig);
    const out = join(dir, 'release');
    const listed = execFileSync('node', [new URL('./release-manifest.mjs', import.meta.url).pathname, '--version', '2.13.0', '--artifacts', dl, '--repo', 'o/n', '--out', out], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
      .trim().split('\n');
    const sums = listed.filter((p) => p.endsWith('SHA256SUMS.txt'));
    expect(sums).toEqual([join(out, 'SHA256SUMS.txt')]);
    const text = readFileSync(sums[0], 'utf8');
    for (const name of ['C_2.13.0_x64-setup.exe', 'C_2.13.0_universal.dmg', 'C.app.tar.gz']) expect(text).toMatch(new RegExp(`^[0-9a-f]{64}  ${name.replace(/\./g, '\\.')}$`, 'm'));
  });
});
