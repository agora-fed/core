// Client for the optional groups module. The server enforces every rule; `canManage` and
// friends only decide what to show.
import { apiDelete, apiGetCredentialed, apiPatch, apiPost } from './api';

export interface GroupDto {
  id: string;
  slug: string;
  name: string;
  description: string;
  entry: 'open' | 'approval';
  forum_path: string | null;
  member_count: number;
  my_status: 'active' | 'pending' | null;
  my_role: 'coordinator' | 'member' | null;
}

export interface GroupMemberDto {
  citizen_id: string;
  handle: string | null;
  display_name: string | null;
  role: 'coordinator' | 'member';
  status: 'active' | 'pending';
}

export interface GroupPersonDto {
  citizen_id: string;
  handle: string | null;
  display_name: string | null;
}

const base = (id: string) => `/api/v1/groups/${encodeURIComponent(id)}`;

export const canManage = (group: GroupDto, isAdmin: boolean): boolean =>
  isAdmin || (group.my_status === 'active' && group.my_role === 'coordinator');

export const canSeeMembers = (group: GroupDto, isAdmin: boolean): boolean =>
  isAdmin || group.my_status === 'active';

export const listGroups = () => apiGetCredentialed<GroupDto[]>('/api/v1/groups');
export const getGroup = (slug: string) =>
  apiGetCredentialed<GroupDto>(`/api/v1/groups/by-slug/${encodeURIComponent(slug)}`);
export const createGroup = (payload: {
  slug: string;
  name: string;
  description: string;
  entry: 'open' | 'approval';
}) => apiPost<{ id: string; slug: string }>('/api/v1/groups', payload);
export const joinGroup = (id: string) => apiPost<{ status: string }>(`${base(id)}/join`, {});
export const leaveGroup = (id: string) => apiDelete<{ ok: true }>(`${base(id)}/join`);
export const archiveGroup = (id: string) => apiDelete<{ ok: true }>(base(id));
export const updateGroup = (id: string, payload: { entry?: 'open' | 'approval' }) =>
  apiPatch<{ ok: true }>(base(id), payload);
export const groupMembers = (id: string) =>
  apiGetCredentialed<GroupMemberDto[]>(`${base(id)}/members`);
export const addGroupMember = (id: string, citizenId: string) =>
  apiPost<{ ok: true }>(`${base(id)}/members`, { citizen_id: citizenId });
export const updateGroupMember = (
  id: string,
  citizenId: string,
  payload: { status?: 'active'; role?: 'coordinator' | 'member' },
) => apiPatch<{ ok: true }>(`${base(id)}/members/${encodeURIComponent(citizenId)}`, payload);
export const removeGroupMember = (id: string, citizenId: string) =>
  apiDelete<{ ok: true }>(`${base(id)}/members/${encodeURIComponent(citizenId)}`);
export const searchPeople = (id: string, q: string) =>
  apiGetCredentialed<GroupPersonDto[]>(`${base(id)}/people?q=${encodeURIComponent(q)}`);
export const enableGroupForum = (id: string) =>
  apiPost<{ forum_path: string }>(`${base(id)}/forum`, {});
