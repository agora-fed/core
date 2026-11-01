import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import {
  isModuleOff,
  moduleFlagKey,
  politicalPackageState,
  toggleableModules,
  visibleForModules,
  type ModuleState,
} from '../src/lib/modules';

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

describe('admin module lists', () => {
  const mod = (id: string, active: boolean, extra: Partial<ModuleState> = {}): ModuleState => ({
    id,
    title: id,
    active,
    core: false,
    gateable: true,
    ...extra,
  });

  it('offers only optional modules outside the political package', () => {
    const list = [
      mod('auth', true, { core: true, gateable: false }),
      mod('forums', true),
      mod('votes', true, { gateable: false }),
      mod('scorecard', false),
    ];
    expect(toggleableModules(list).map((m) => m.id)).toEqual(['forums']);
  });

  it('lists what is missing from the political package', () => {
    const state = politicalPackageState([
      mod('mandates', false, { gateable: false }),
      mod('consequence', true, { gateable: false }),
      mod('forums', true),
    ]);
    expect(state).toEqual({ installed: false, missing: ['mandates'] });
  });

  it('reports the package as installed when every member is on', () => {
    expect(politicalPackageState([mod('mandates', true), mod('proposals', true)]).installed).toBe(true);
  });

  it('builds the flag key the gateway reads', () => {
    expect(moduleFlagKey('forums')).toBe('module.forums');
  });
});

describe('settings tabs', () => {
  const shell = readFileSync('src/components/islands/SettingsShell.svelte', 'utf8');
  it('declares the voter-ID and fediverse tabs as module-gated', () => {
    expect(shell).toContain("label: 'Identidade', requires: 'mandates'");
    expect(shell).toContain("label: 'Fediverso', requires: 'federation'");
  });
});
