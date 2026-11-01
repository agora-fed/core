import { apiGet, apiPost, apiPut, DEFAULT_ORG_ID } from './api';

export interface MeetingDto {
  id: string;
  org_id: string;
  title: string;
  location: string;
  starts_at: string;
  created_at: string;
}

export interface AttendeeDto {
  id: string;
  meeting_id: string;
  citizen_id: string;
  created_at: string;
}

export const ATTENDEE_PAGE = 100;

export const listMeetings = () =>
  apiGet<MeetingDto[]>(`/api/v1/meetings?org_id=${DEFAULT_ORG_ID}&limit=100`);
export const getMeeting = (id: string) =>
  apiGet<MeetingDto>(`/api/v1/meetings/${encodeURIComponent(id)}`);
export const listAttendees = (id: string) =>
  apiGet<AttendeeDto[]>(
    `/api/v1/meetings/${encodeURIComponent(id)}/attendees?limit=${ATTENDEE_PAGE}`,
  );
export const scheduleMeeting = (body: { title: string; location: string; starts_at: string }) =>
  apiPost<MeetingDto>('/api/v1/meetings', body);
export const attendMeeting = (id: string) =>
  apiPost<AttendeeDto>(`/api/v1/meetings/${encodeURIComponent(id)}/attendees`, {});
export const getMinutes = (id: string) =>
  apiGet<{ minutes: string | null }>(`/api/v1/meetings/${encodeURIComponent(id)}/minutes`);
export const setMinutes = (id: string, minutes: string) =>
  apiPut<{ minutes: string | null }>(`/api/v1/meetings/${encodeURIComponent(id)}/minutes`, { minutes });

/** Upcoming first by nearest date, then past meetings by most recent. */
export function splitMeetings(meetings: MeetingDto[], now: Date) {
  const time = (m: MeetingDto) => new Date(m.starts_at).getTime();
  const upcoming = meetings
    .filter((m) => time(m) >= now.getTime())
    .sort((a, b) => time(a) - time(b));
  const past = meetings
    .filter((m) => time(m) < now.getTime())
    .sort((a, b) => time(b) - time(a));
  return { upcoming, past };
}

/** A `datetime-local` value as UTC ISO, or null when empty or invalid. */
export function localToIso(value: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export function attendeeLabel(count: number): string {
  if (count >= ATTENDEE_PAGE) return `Mais de ${ATTENDEE_PAGE - 1} presenças confirmadas`;
  if (count === 1) return '1 presença confirmada';
  return `${count} presenças confirmadas`;
}

export const formatMeetingDate = (iso: string) =>
  new Date(iso).toLocaleString('pt-BR', { dateStyle: 'long', timeStyle: 'short' });
