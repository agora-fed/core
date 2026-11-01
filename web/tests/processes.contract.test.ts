import { describe, expect, it } from 'vitest';
import {
  nextPhasePosition,
  slugify,
  sortPhases,
  statusLabel,
  type PhaseDto,
} from '../src/lib/processes';

const phase = (id: string, position: number): PhaseDto => ({
  id,
  process_id: 'p',
  name: id,
  position,
  active: false,
  created_at: '2030-01-01T00:00:00Z',
});

describe('slugify', () => {
  it('strips accents and punctuation', () => {
    expect(slugify('  Revisão do Estatuto, 2031! ')).toBe('revisao-do-estatuto-2031');
  });
});

describe('phases', () => {
  it('sorts without touching the input', () => {
    const input = [phase('b', 1), phase('a', 0)];
    expect(sortPhases(input).map((p) => p.id)).toEqual(['a', 'b']);
    expect(input[0].id).toBe('b');
  });
  it('appends after the last position', () => {
    expect(nextPhasePosition([])).toBe(0);
    expect(nextPhasePosition([phase('a', 2)])).toBe(3);
  });
});

describe('statusLabel', () => {
  it('translates known values and keeps unknown ones', () => {
    expect(statusLabel('active')).toBe('Em andamento');
    expect(statusLabel('other')).toBe('other');
  });
});
