<script lang="ts">
  // Amendments (Decidim gap): variants proposed by other citizens for a given
  // proposal. Shows a list with a diff-lite view (green added / red removed
  // computed client-side), a compose form for a new draft, and lifecycle
  // actions (Publish, Withdraw for author; Accept, Reject for proposal author).
  import { onMount } from 'svelte';
  import {
    listAmendments,
    createAmendment,
    publishAmendment,
    acceptAmendment,
    rejectAmendment,
    type AmendmentDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import Alert from '../ui/Alert.svelte';
  import Icon from '../ui/Icon.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Textarea from '../ui/Textarea.svelte';

  interface Props {
    proposalId: string;
    proposalBody: string;
    proposalAuthorId?: string | null;
  }
  let { proposalId, proposalBody, proposalAuthorId = null }: Props = $props();

  let currentCitizen = $state<string | null>(null);
  let amendments = $state<AmendmentDto[]>([]);
  let loading = $state(true);
  let loadErr = $state<string | null>(null);

  let showCompose = $state(false);
  let newBody = $state('');
  let newRationale = $state('');
  let creating = $state(false);
  let createErr = $state<string | null>(null);

  let busyId = $state<string | null>(null);

  function readCitizen() {
    try {
      currentCitizen = localStorage.getItem('dsoc_citizen');
    } catch {
      /* storage may be blocked */
    }
  }

  async function reload() {
    loading = true;
    loadErr = null;
    const res = await listAmendments(proposalId);
    loading = false;
    if (res.success && res.data) {
      amendments = res.data;
    } else {
      loadErr = res.error?.message ?? 'Falha ao carregar emendas.';
    }
  }

  onMount(() => {
    readCitizen();
    reload();
  });

  async function submitNew() {
    if (!newBody.trim() || creating) return;
    creating = true;
    createErr = null;
    // Create draft, then immediately publish. Users don't need a
    // "keep as draft" flow in this first pass.
    const res = await createAmendment(proposalId, newBody, newRationale || undefined);
    if (!res.success || !res.data) {
      creating = false;
      createErr = res.error?.message ?? 'Não foi possível criar a emenda.';
      return;
    }
    const pub = await publishAmendment(res.data.id);
    creating = false;
    if (pub.success) {
      newBody = '';
      newRationale = '';
      showCompose = false;
      await reload();
    } else {
      createErr = pub.error?.message ?? 'Emenda criada mas não foi publicada.';
    }
  }

  async function accept(a: AmendmentDto) {
    if (!confirm('Aceitar essa emenda? A proposta será atualizada com o novo texto.')) return;
    busyId = a.id;
    const res = await acceptAmendment(a.id);
    busyId = null;
    if (res.success) await reload();
    else alert(res.error?.message ?? 'Falha ao aceitar.');
  }
  async function reject(a: AmendmentDto) {
    if (!confirm('Rejeitar essa emenda?')) return;
    busyId = a.id;
    const res = await rejectAmendment(a.id);
    busyId = null;
    if (res.success) await reload();
    else alert(res.error?.message ?? 'Falha ao rejeitar.');
  }

  // Word-level diff for the tiny inline preview. Not production-grade; good
  // enough to show what changed against the current proposal body. For long
  // texts we cap the tokens rendered.
  function diffWords(a: string, b: string): { kind: 'same' | 'add' | 'del'; text: string }[] {
    const aWords = a.split(/(\s+)/);
    const bWords = b.split(/(\s+)/);
    const out: { kind: 'same' | 'add' | 'del'; text: string }[] = [];
    let i = 0;
    let j = 0;
    while (i < aWords.length || j < bWords.length) {
      if (i >= aWords.length) {
        out.push({ kind: 'add', text: bWords[j++] });
        continue;
      }
      if (j >= bWords.length) {
        out.push({ kind: 'del', text: aWords[i++] });
        continue;
      }
      if (aWords[i] === bWords[j]) {
        out.push({ kind: 'same', text: aWords[i] });
        i++;
        j++;
        continue;
      }
      // simple lookahead: is aWords[i] later in b?
      const skipInB = bWords.slice(j, j + 8).indexOf(aWords[i]);
      const skipInA = aWords.slice(i, i + 8).indexOf(bWords[j]);
      if (skipInB >= 0 && (skipInA < 0 || skipInB <= skipInA)) {
        for (let k = 0; k < skipInB; k++) out.push({ kind: 'add', text: bWords[j++] });
      } else if (skipInA >= 0) {
        for (let k = 0; k < skipInA; k++) out.push({ kind: 'del', text: aWords[i++] });
      } else {
        out.push({ kind: 'del', text: aWords[i++] });
        out.push({ kind: 'add', text: bWords[j++] });
      }
    }
    return out;
  }

  const statusLabel: Record<AmendmentDto['status'], string> = {
    draft: 'rascunho',
    open: 'em votação',
    accepted: 'aceita',
    rejected: 'rejeitada',
    withdrawn: 'retirada',
  };
  const statusTone: Record<AmendmentDto['status'], 'neutral' | 'success' | 'warning' | 'danger'> = {
    draft: 'neutral',
    open: 'warning',
    accepted: 'success',
    rejected: 'danger',
    withdrawn: 'neutral',
  };

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleString('pt-BR', {
        day: '2-digit',
        month: 'short',
        year: 'numeric',
      });
    } catch {
      return iso;
    }
  }
</script>

<section class="wrap">
  <header class="head">
    <h2><Icon name="edit" size={18} /> Emendas</h2>
    <p class="muted">
      Variantes de texto propostas por outras pessoas. A autoria original pode
      aceitar (o texto substitui a proposta e vira revisão) ou rejeitar.
    </p>
    {#if currentCitizen}
      <Button
        variant={showCompose ? 'ghost' : 'primary'}
        size="sm"
        onclick={() => (showCompose = !showCompose)}
      >
        {showCompose ? 'Cancelar' : 'Propor emenda'}
      </Button>
    {/if}
  </header>

  {#if showCompose && currentCitizen}
    <Card>
      <h3 class="sub">Nova emenda</h3>
      <Textarea
        id="amend-body"
        label="Novo texto da proposta"
        rows={6}
        bind:value={newBody}
        placeholder={proposalBody.slice(0, 200)}
      />
      <Textarea
        id="amend-rationale"
        label="Justificativa (opcional)"
        rows={3}
        bind:value={newRationale}
        placeholder="Por que essa mudança melhora a proposta?"
      />
      {#if createErr}
        <div class="alert-slot"><Alert tone="danger">{createErr}</Alert></div>
      {/if}
      <div class="actions">
        <Button
          variant="primary"
          onclick={submitNew}
          loading={creating}
          disabled={!newBody.trim()}
        >
          Publicar emenda
        </Button>
      </div>
    </Card>
  {/if}

  {#if loading}
    <div class="loading"><Spinner /></div>
  {:else if loadErr}
    <Alert tone="danger">{loadErr}</Alert>
  {:else if amendments.length === 0}
    <Card padding="none">
      <EmptyState
        icon="edit"
        title="Nenhuma emenda ainda"
        description="Ninguém propôs uma variante desta proposta. Você pode ser a primeira."
      />
    </Card>
  {:else}
    <ul class="list">
      {#each amendments as a (a.id)}
        <li>
          <Card>
            <div class="amend-head">
              <div class="who">
                <strong>{a.author_display_name ?? a.author_handle ?? 'Alguém'}</strong>
                {#if a.author_handle}
                  <span class="muted">@{a.author_handle}</span>
                {/if}
                <span class="muted">· {fmtDate(a.created_at)}</span>
              </div>
              <Badge tone={statusTone[a.status]}>{statusLabel[a.status]}</Badge>
            </div>
            <div class="diff">
              {#each diffWords(proposalBody, a.body).slice(0, 400) as tok}
                {#if tok.kind === 'add'}
                  <ins>{tok.text}</ins>
                {:else if tok.kind === 'del'}
                  <del>{tok.text}</del>
                {:else}
                  <span>{tok.text}</span>
                {/if}
              {/each}
            </div>
            {#if a.rationale}
              <div class="rationale">
                <span class="muted">Justificativa:</span>
                <p>{a.rationale}</p>
              </div>
            {/if}
            <div class="foot">
              <span class="muted">
                <Icon name="heart" size={12} /> {a.support_count}
              </span>
              {#if a.status === 'open' && currentCitizen === proposalAuthorId}
                <div class="row-actions">
                  <Button
                    variant="ghost"
                    size="sm"
                    onclick={() => reject(a)}
                    disabled={busyId === a.id}
                  >
                    Rejeitar
                  </Button>
                  <Button
                    variant="primary"
                    size="sm"
                    onclick={() => accept(a)}
                    loading={busyId === a.id}
                  >
                    Aceitar
                  </Button>
                </div>
              {/if}
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .wrap {
    display: grid;
    gap: var(--sp-3);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .head h2 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .head p {
    margin: 0;
    flex: 1;
    min-width: 200px;
    font-size: var(--fs-sm);
  }
  .sub {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-base);
    color: var(--text-1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: var(--sp-3);
  }
  .alert-slot {
    margin-top: var(--sp-3);
  }
  .loading {
    display: flex;
    justify-content: center;
    padding: var(--sp-5);
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .amend-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    margin-bottom: var(--sp-3);
    flex-wrap: wrap;
  }
  .who {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    flex-wrap: wrap;
  }
  .who strong {
    color: var(--text-1);
  }
  .diff {
    padding: var(--sp-3);
    background: var(--surface-2);
    border-radius: var(--r-sm);
    line-height: 1.6;
    font-size: var(--fs-sm);
    white-space: pre-wrap;
    overflow-wrap: break-word;
  }
  .diff ins {
    background: color-mix(in oklab, var(--positive, #22c55e) 25%, transparent);
    text-decoration: none;
    padding: 0 2px;
    border-radius: var(--r-xs);
  }
  .diff del {
    background: color-mix(in oklab, var(--negative, #ef4444) 25%, transparent);
    text-decoration: line-through;
    padding: 0 2px;
    border-radius: var(--r-xs);
  }
  .rationale {
    margin-top: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    border-left: 3px solid var(--border-subtle);
  }
  .rationale p {
    margin: 4px 0 0;
    color: var(--text-1);
    font-size: var(--fs-sm);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }
  .foot .muted {
    font-size: var(--fs-xs);
  }
  .row-actions {
    display: flex;
    gap: var(--sp-2);
  }
</style>
