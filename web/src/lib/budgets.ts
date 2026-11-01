import { apiGet, apiPost, DEFAULT_ORG_ID } from './api';

export interface BudgetDto {
  id: string;
  org_id: string;
  title: string;
  ceiling_cents: number;
  created_at: string;
}

export interface ProjectDto {
  id: string;
  budget_id: string;
  title: string;
  cost_cents: number;
  created_at: string;
}

const budgetPath = (id: string) => `/api/v1/budgets/${encodeURIComponent(id)}`;

export const listBudgets = () =>
  apiGet<BudgetDto[]>(`/api/v1/budgets?org_id=${DEFAULT_ORG_ID}&limit=100`);
export const getBudget = (id: string) => apiGet<BudgetDto>(budgetPath(id));
export const listProjects = (id: string) =>
  apiGet<ProjectDto[]>(`${budgetPath(id)}/projects?limit=100`);
export const createBudget = (title: string, ceiling_cents: number) =>
  apiPost<BudgetDto>('/api/v1/budgets', { title, ceiling_cents });
export const addProject = (id: string, title: string, cost_cents: number) =>
  apiPost<ProjectDto>(`${budgetPath(id)}/projects`, { title, cost_cents });
export const placeOrder = (id: string, project_ids: string[]) =>
  apiPost<unknown>(`${budgetPath(id)}/orders`, { project_ids });

export const formatCents = (cents: number) =>
  (cents / 100).toLocaleString('pt-BR', { style: 'currency', currency: 'BRL' });

/** Reais typed by a person as cents, or null when not a positive amount. */
export function reaisToCents(value: string): number | null {
  const normalized = value.trim().replace(/\./g, '').replace(',', '.');
  if (!/^\d+(\.\d{1,2})?$/.test(normalized)) return null;
  const cents = Math.round(Number(normalized) * 100);
  return cents > 0 ? cents : null;
}

export function allocatedCents(projects: ProjectDto[], selected: string[]): number {
  return projects
    .filter((p) => selected.includes(p.id))
    .reduce((sum, p) => sum + p.cost_cents, 0);
}

export const toggleProject = (selected: string[], id: string) =>
  selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id];

export const canPlaceOrder = (allocated: number, ceiling: number) =>
  allocated > 0 && allocated <= ceiling;
