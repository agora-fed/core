import { describe, expect, it } from 'vitest';
import { buildAnswers, nextPosition, sortQuestions, type QuestionDto } from '../src/lib/surveys';

const question = (id: string, position: number, kind = 'text'): QuestionDto => ({
  id,
  survey_id: 's',
  prompt: id,
  kind,
  position,
  created_at: '2030-01-01T00:00:00Z',
});

describe('question order', () => {
  it('sorts by position without touching the input', () => {
    const input = [question('b', 1), question('a', 0)];
    expect(sortQuestions(input).map((q) => q.id)).toEqual(['a', 'b']);
    expect(input[0].id).toBe('b');
  });
  it('appends after the last position', () => {
    expect(nextPosition([])).toBe(0);
    expect(nextPosition([question('a', 4)])).toBe(5);
  });
});

describe('buildAnswers', () => {
  const questions = [question('t', 0), question('b', 1, 'boolean')];

  it('requires every question', () => {
    expect(buildAnswers(questions, { t: 'oi' })).toBeNull();
    expect(buildAnswers(questions, { t: '  ', b: 'yes' })).toBeNull();
  });
  it('types yes/no answers as booleans', () => {
    expect(buildAnswers(questions, { t: ' oi ', b: 'no' })).toEqual({ t: 'oi', b: false });
  });
});
