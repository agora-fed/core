import { describe, expect, it } from 'vitest';
import { isModuleOff, visibleForModules } from '../src/lib/modules';

const root = (modulesOff?: string) => ({ dataset: { modulesOff } }) as { dataset: DOMStringMap };

describe('isModuleOff', () => {
  it('treats a missing attribute as every module on', () => {
    expect(isModuleOff('mandates', root())).toBe(false);
  });
  it('matches whole ids only', () => {
    expect(isModuleOff('mandates', root('mandates proposals'))).toBe(true);
    expect(isModuleOff('mandate', root('mandates proposals'))).toBe(false);
  });
});

describe('visibleForModules', () => {
  const items = [
    { id: 'always' },
    { id: 'political', requires: 'mandates' },
    { id: 'replacement', whenOff: 'mandates' },
  ];
  const ids = (off: string[]) =>
    visibleForModules(items, (m) => off.includes(m)).map((i) => i.id);

  it('keeps the political list untouched when the module is on', () => {
    expect(ids([])).toEqual(['always', 'political']);
  });
  it('swaps entries when the module is off', () => {
    expect(ids(['mandates'])).toEqual(['always', 'replacement']);
  });
});
