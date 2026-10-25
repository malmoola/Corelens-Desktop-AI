/**
 * Authenticode signing, with no certificate of this product's own yet.
 *
 * No certificate travels with this product but one it publishes itself:
 * not inside the installer, and not beside it unless it is under
 * `brand/signing/` (the build copies what it finds there and refuses a
 * signing secret that is not one of them). When there is one, the chain is
 * checked here.
 */
import { X509Certificate } from 'node:crypto';
import { existsSync, readdirSync, readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const at = (name) => new URL(`../${name}`, import.meta.url);
const text = (name) => readFileSync(at(name), 'utf8');

describe('signing', () => {
  it('ships no certificate inside the installer', () => {
    const conf = JSON.parse(text('src-tauri/tauri.conf.json'));
    const shipped = [...Object.keys(conf.bundle.resources), ...Object.values(conf.bundle.resources)];
    expect(shipped.filter((p) => /\.(crt|cer|pem|pfx|p12)$/i.test(p))).toEqual([]);
  });

  it('keeps no certificate in brand/ outside brand/signing/', () => {
    const loose = readdirSync(at('brand/')).filter((n) => /\.(crt|cer|pem|pfx|p12)$/i.test(n));
    expect(loose).toEqual([]);
  });

  it('checks a published chain, when there is one', () => {
    const dir = at('brand/signing/');
    const certs = existsSync(dir)
      ? readdirSync(dir).filter((n) => n.endsWith('.crt')).map((n) => new X509Certificate(readFileSync(new URL(n, dir))))
      : [];
    const roots = certs.filter((c) => c.ca);
    for (const leaf of certs.filter((c) => !c.ca)) {
      expect(leaf.keyUsage).toContain('1.3.6.1.5.5.7.3.3'); // id-kp-codeSigning
      expect(roots.some((r) => leaf.checkIssued(r) && leaf.verify(r.publicKey)), leaf.subject).toBe(true);
      expect(new Date(leaf.validTo).getTime()).toBeGreaterThan(Date.now());
    }
  });
});
