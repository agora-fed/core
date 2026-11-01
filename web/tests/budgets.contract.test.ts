import { describe, expect, it } from 'vitest';
import {
  allocatedCents,
  canPlaceOrder,
  reaisToCents,
  toggleProject,
  type ProjectDto,
} from '../src/lib/budgets';

const project = (id: string, cost_cents: number): ProjectDto => ({
  id,
  budget_id: 'b',
  title: id,
  cost_cents,
  created_at: '2030-01-01T00:00:00Z',
});

describe('reaisToCents', () => {
  it('reads Brazilian formatting', () => {
    expect(reaisToCents('1.234,50')).toBe(123450);
    expect(reaisToCents('10')).toBe(1000);
  });
  it('rejects zero and malformed amounts', () => {
    expect(reaisToCents('0')).toBeNull();
    expect(reaisToCents('abc')).toBeNull();
    expect(reaisToCents('1,234')).toBeNull();
  });
});

describe('selection', () => {
  it('sums only selected projects', () => {
    expect(allocatedCents([project('a', 100), project('b', 50)], ['b'])).toBe(50);
  });
  it('toggles without mutating', () => {
    const start = ['a'];
    expect(toggleProject(start, 'b')).toEqual(['a', 'b']);
    expect(toggleProject(start, 'a')).toEqual([]);
    expect(start).toEqual(['a']);
  });
  it('allows an order up to the ceiling only', () => {
    expect(canPlaceOrder(0, 100)).toBe(false);
    expect(canPlaceOrder(100, 100)).toBe(true);
    expect(canPlaceOrder(101, 100)).toBe(false);
  });
});
