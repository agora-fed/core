import { describe, expect, it } from 'vitest';
import { attendeeLabel, localToIso, splitMeetings, type MeetingDto } from '../src/lib/meetings';

const meeting = (id: string, starts_at: string): MeetingDto => ({
  id,
  org_id: 'o',
  title: id,
  location: 'x',
  starts_at,
  created_at: starts_at,
});

describe('splitMeetings', () => {
  it('orders upcoming by nearest and past by most recent', () => {
    const { upcoming, past } = splitMeetings(
      [
        meeting('far', '2030-02-01T00:00:00Z'),
        meeting('old', '2020-01-01T00:00:00Z'),
        meeting('near', '2030-01-01T00:00:00Z'),
        meeting('recent', '2021-01-01T00:00:00Z'),
      ],
      new Date('2025-01-01T00:00:00Z'),
    );
    expect(upcoming.map((m) => m.id)).toEqual(['near', 'far']);
    expect(past.map((m) => m.id)).toEqual(['recent', 'old']);
  });
});

describe('localToIso', () => {
  it('rejects empty and invalid values', () => {
    expect(localToIso('')).toBeNull();
    expect(localToIso('not-a-date')).toBeNull();
  });
  it('produces an ISO timestamp', () => {
    expect(localToIso('2030-01-01T10:00')).toMatch(/^2030-01-01T\d\d:00:00\.000Z$/);
  });
});

describe('attendeeLabel', () => {
  it('handles singular, plural and the page cap', () => {
    expect(attendeeLabel(1)).toBe('1 presença confirmada');
    expect(attendeeLabel(2)).toBe('2 presenças confirmadas');
    expect(attendeeLabel(100)).toBe('Mais de 99 presenças confirmadas');
  });
});
