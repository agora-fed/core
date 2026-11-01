import { apiGet, apiPost, DEFAULT_ORG_ID } from './api';

export interface SurveyDto {
  id: string;
  org_id: string;
  title: string;
  published_at: string | null;
  created_at: string;
}

export interface QuestionDto {
  id: string;
  survey_id: string;
  prompt: string;
  kind: string;
  position: number;
  created_at: string;
}

export const YES_NO_KIND = 'boolean';

const surveyPath = (id: string) => `/api/v1/surveys/${encodeURIComponent(id)}`;

export const listSurveys = () =>
  apiGet<SurveyDto[]>(`/api/v1/surveys?org_id=${DEFAULT_ORG_ID}&limit=100`);
export const getSurvey = (id: string) => apiGet<SurveyDto>(surveyPath(id));
export const listQuestions = (id: string) =>
  apiGet<QuestionDto[]>(`${surveyPath(id)}/questions?limit=100`);
export const createSurvey = (title: string) =>
  apiPost<SurveyDto>('/api/v1/surveys', { title });
export const addQuestion = (id: string, body: { prompt: string; kind: string; position: number }) =>
  apiPost<QuestionDto>(`${surveyPath(id)}/questions`, body);
export const publishSurvey = (id: string) => apiPost<SurveyDto>(`${surveyPath(id)}/publish`, {});
export const submitResponse = (id: string, answers: Record<string, string | boolean>) =>
  apiPost<unknown>(`${surveyPath(id)}/responses`, { answers });

export const sortQuestions = (questions: QuestionDto[]) =>
  [...questions].sort((a, b) => a.position - b.position);

export const nextPosition = (questions: QuestionDto[]) =>
  questions.reduce((max, q) => Math.max(max, q.position + 1), 0);

/** The answers payload, or null while any question is unanswered. */
export function buildAnswers(
  questions: QuestionDto[],
  draft: Record<string, string>,
): Record<string, string | boolean> | null {
  const answers: Record<string, string | boolean> = {};
  for (const q of questions) {
    const value = (draft[q.id] ?? '').trim();
    if (!value) return null;
    answers[q.id] = q.kind === YES_NO_KIND ? value === 'yes' : value;
  }
  return answers;
}
