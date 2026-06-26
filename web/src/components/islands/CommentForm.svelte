<script lang="ts">
  // Comment form for deliberating a proposal.
  import { apiPost, DEFAULT_ORG_ID } from '../../lib/api';

  interface Props {
    proposalId: string;
    orgId?: string;
  }

  let { proposalId, orgId = DEFAULT_ORG_ID }: Props = $props();

  let body = $state('');
  let busy = $state(false);
  let status = $state<{ kind: 'error' | 'ok' | 'info'; text: string } | null>(
    null,
  );

  const MIN = 3;
  const MAX = 2000;
  let remaining = $derived(MAX - body.length);
  let valid = $derived(body.trim().length >= MIN && body.length <= MAX);

  function readCitizenId(): string | null {
    try {
      return localStorage.getItem('dsoc_citizen');
    } catch {
      return null;
    }
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    status = null;

    const citizenId = readCitizenId();
    if (!citizenId) {
      status = { kind: 'info', text: 'Entre na sua conta para comentar.' };
      return;
    }

    busy = true;
    const res = await apiPost('/api/v1/comments', {
      org_id: orgId,
      citizen_id: citizenId,
      proposal_id: proposalId,
      body: body.trim(),
    });
    busy = false;

    if (res.success) {
      status = {
        kind: 'ok',
        text: 'Comentário enviado. Obrigado por participar!',
      };
      body = '';
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar seu comentário.',
      };
    }
  }
</script>

<form class="comment-form" onsubmit={submit} novalidate>
  <div class="field">
    <label for="comment-body">Seu comentário</label>
    <textarea
      id="comment-body"
      class="input"
      rows="4"
      maxlength={MAX}
      bind:value={body}
      placeholder="Contribua com argumentos, dados ou uma sugestão construtiva…"
      aria-describedby="comment-hint"
    ></textarea>
    <p id="comment-hint" class="hint muted">
      {remaining} caracteres restantes
    </p>
  </div>

  <button class="btn btn-primary" type="submit" disabled={!valid || busy}>
    {busy ? 'Enviando…' : 'Publicar comentário'}
  </button>

  {#if status}
    <p class={`note ${status.kind}`} role="status">
      {status.text}
      {#if status.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
    </p>
  {/if}
</form>

<style>
  .comment-form {
    margin-top: 1rem;
  }
  textarea.input {
    resize: vertical;
    min-height: 96px;
  }
  .note {
    margin: 0.75rem 0 0;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .note.ok {
    color: var(--c-green-dark);
  }
  .note.info {
    color: var(--c-text-muted);
  }
</style>
