<script lang="ts">
  // Support / vote button. Casts an aggregate support signal for a proposal.
  // Individual linkage is never exposed (LGPD); we only reflect the public count.
  import { apiPost, DEFAULT_ORG_ID } from '../../lib/api';
  import type { ProposalDto } from '../../lib/types';

  interface Props {
    proposalId: string;
    orgId?: string;
    initialCount: number;
  }

  let { proposalId, orgId = DEFAULT_ORG_ID, initialCount }: Props = $props();

  let count = $state(initialCount);
  let supported = $state(false);
  let busy = $state(false);
  let message = $state<{ kind: 'error' | 'info'; text: string } | null>(null);

  function readCitizenId(): string | null {
    try {
      return localStorage.getItem('dsoc_citizen');
    } catch {
      return null;
    }
  }

  async function support() {
    if (busy || supported) return;
    busy = true;
    message = null;

    const citizenId = readCitizenId();
    if (!citizenId) {
      message = {
        kind: 'info',
        text: 'Entre na sua conta para apoiar esta proposta.',
      };
      busy = false;
      return;
    }

    // Optimistic update; revert on failure.
    count += 1;
    supported = true;

    const res = await apiPost<ProposalDto>('/api/v1/votes', {
      org_id: orgId,
      proposal_id: proposalId,
      citizen_id: citizenId,
    });

    if (!res.success) {
      count -= 1;
      supported = false;
      message = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível registrar seu apoio.',
      };
    }
    busy = false;
  }
</script>

<div class="vote">
  <button
    class="btn btn-primary"
    onclick={support}
    disabled={busy || supported}
    aria-pressed={supported}
  >
    {#if supported}
      ✓ Apoiado
    {:else}
      Apoiar esta proposta
    {/if}
  </button>
  <p class="count" aria-live="polite">
    <strong>{count.toLocaleString('pt-BR')}</strong>
    {count === 1 ? 'apoio' : 'apoios'}
  </p>
  {#if message}
    <p class={`note ${message.kind}`} role="status">
      {message.text}
      {#if message.kind === 'info'}
        <a href="/entrar">Entrar</a>
      {/if}
    </p>
  {/if}
</div>

<style>
  .vote {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .count {
    margin: 0;
    color: var(--c-text-muted);
  }
  .count strong {
    color: var(--c-navy);
    font-size: 1.25rem;
  }
  .note {
    flex-basis: 100%;
    margin: 0.25rem 0 0;
    font-size: 0.9rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .note.info {
    color: var(--c-text-muted);
  }
</style>
