import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { progress, proofFromQuery, solveAltcha, type AltchaChallenge } from '../src/lib/petitions';

const sha = (s: string) => createHash('sha256').update(s).digest('hex');

function challenge(number: number, max = 500): AltchaChallenge {
  const salt = 'abc?expires=1800000000';
  return { algorithm: 'SHA-256', challenge: sha(`${salt}${number}`), maxnumber: max, salt, signature: 'sig' };
}

describe('progress', () => {
  it('is a bounded percentage', () => {
    expect(progress(0, 100)).toBe(0);
    expect(progress(50, 200)).toBe(25);
    expect(progress(500, 100)).toBe(100);
    expect(progress(3, 0)).toBe(100);
  });
});

describe('proofFromQuery', () => {
  it('reads the local-mode token', () => {
    expect(proofFromQuery('?t=abc')).toEqual({ token: 'abc' });
  });
  it('reads the sso-mode code', () => {
    expect(proofFromQuery('?p=1&u=2&c=XY')).toEqual({ pending: '1', user: '2', code: 'XY' });
  });
  it('refuses an incomplete link', () => {
    expect(proofFromQuery('?p=1&u=2')).toBeNull();
    expect(proofFromQuery('')).toBeNull();
  });
});

describe('solveAltcha', () => {
  it('finds the number and returns the payload the server expects', async () => {
    const payload = await solveAltcha(challenge(321));
    expect(payload).not.toBeNull();
    const decoded = JSON.parse(atob(payload as string));
    expect(decoded).toEqual({
      algorithm: 'SHA-256',
      challenge: sha('abc?expires=1800000000321'),
      number: 321,
      salt: 'abc?expires=1800000000',
      signature: 'sig',
    });
  });
  it('gives up on an unsolvable or unknown challenge', async () => {
    expect(await solveAltcha(challenge(600, 100))).toBeNull();
    expect(await solveAltcha({ ...challenge(1), algorithm: 'SHA-1' })).toBeNull();
  });
});
