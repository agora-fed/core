import { describe, expect, it } from 'vitest';
import { loginUrl, requiresSignIn } from '../src/lib/session';

describe('requiresSignIn', () => {
  it('covers private areas and their subpages', () => {
    expect(requiresSignIn('/admin')).toBe(true);
    expect(requiresSignIn('/admin/usuarios/')).toBe(true);
    expect(requiresSignIn('/configuracoes')).toBe(true);
  });
  it('leaves public pages and lookalike paths alone', () => {
    expect(requiresSignIn('/')).toBe(false);
    expect(requiresSignIn('/f/geral')).toBe(false);
    expect(requiresSignIn('/administracao')).toBe(false);
  });
});

describe('loginUrl', () => {
  it('returns to the same page after signing in', () => {
    expect(loginUrl('/admin/modulos/', '?x=1')).toBe('/entrar/?next=%2Fadmin%2Fmodulos%2F%3Fx%3D1');
  });
});
