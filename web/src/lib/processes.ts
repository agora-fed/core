import { apiGet, apiPost, DEFAULT_ORG_ID } from './api';

export interface ProcessDto {
  id: string;
  org_id: string;
  slug: string;
  title: string;
  status: string;
  starts_at: string | null;
  ends_at: string | null;
  created_at: string;
}

export interface PhaseDto {
  id: string;
  process_id: string;
  name: string;
  position: number;
  active: boolean;
  created_at: string;
}

const processPath = (id: string) => `/api/v1/processes/${encodeURIComponent(id)}`;

export const listProcesses = () =>
  apiGet<ProcessDto[]>(`/api/v1/processes?org_id=${DEFAULT_ORG_ID}&limit=100`);
export const getProcess = (id: string) =>
  apiGet<ProcessDto>(`${processPath(id)}?org_id=${DEFAULT_ORG_ID}`);
export const listPhases = (id: string) =>
  apiGet<PhaseDto[]>(`${processPath(id)}/phases?limit=100`);
export const createProcess = (title: string) =>
  apiPost<ProcessDto>('/api/v1/processes', { title, slug: slugify(title) });
export const addPhase = (id: string, name: string, position: number) =>
  apiPost<PhaseDto>(`${processPath(id)}/phases`, { name, position });
export const advancePhase = (id: string) =>
  apiPost<PhaseDto>(`${processPath(id)}/phases/advance`, {});

const STATUS_LABELS: Record<string, string> = {
  draft: 'Rascunho',
  active: 'Em andamento',
  closed: 'Encerrado',
};

export const statusLabel = (status: string) => STATUS_LABELS[status] ?? status;

export function slugify(title: string): string {
  return title
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

export const sortPhases = (phases: PhaseDto[]) =>
  [...phases].sort((a, b) => a.position - b.position);

export const nextPhasePosition = (phases: PhaseDto[]) =>
  phases.reduce((max, p) => Math.max(max, p.position + 1), 0);
