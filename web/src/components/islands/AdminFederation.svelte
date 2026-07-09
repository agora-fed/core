<script lang="ts">
  // /admin/federacao — bloqueios de domínio server-wide.
  //
  // Tabela + form pra adicionar. Delete inline em cada linha. Uso o pattern
  // otimista (adiciona/remove local antes da resposta, reverte em erro).
  import { onMount } from 'svelte';
  import {
    adminListDomainBlocks,
    adminAddDomainBlock,
    adminRemoveDomainBlock,
    type AdminDomainBlockDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let rows = $state<AdminDomainBlockDto[]>([]);
  let removingBusy = $state<Set<string>>(new Set());

  // Form state
  let domainInput = $state('');
  let severityInput = $state<'silence' | 'suspend'>('silence');
  let reasonInput = $state('');
  let adding = $state(false);

  async function reload() {
    loading = true;
    error = null;
    const res = await adminListDomainBlocks();
    loading = false;
    if (res.success && res.data) {
      rows = res.data;
    } else {
      error = res.error?.message ?? 'Falha ao carregar bloqueios.';
    }
  }

  onMount(reload);

  async function onAdd(e: SubmitEvent) {
    e.preventDefault();
    const d = domainInput.trim();
    if (!d) return;
    adding = true;
    const res = await adminAddDomainBlock(d, severityInput, reasonInput.trim() || undefined);
    adding = false;
    if (res.success) {
      toast.success(`Domínio ${d} bloqueado (${severityInput}).`);
      domainInput = '';
      reasonInput = '';
      severityInput = 'silence';
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha ao bloquear.');
    }
  }

  async function onRemove(domain: string) {
    if (removingBusy.has(domain)) return;
    if (!confirm(`Remover bloqueio de ${domain}?`)) return;
    removingBusy = new Set(removingBusy).add(domain);
    const res = await adminRemoveDomainBlock(domain);
    const done = new Set(removingBusy);
    done.delete(domain);
    removingBusy = done;
    if (res.success) {
      toast.success(`Bloqueio de ${domain} removido.`);
      rows = rows.filter((r) => r.domain !== domain);
    } else {
      toast.error(res.error?.message ?? 'Falha ao remover.');
    }
  }

  function severityLabel(s: string) {
    return s === 'suspend' ? 'Suspenso (corte total)' : 'Silenciado';
  }
  function severityTone(s: string): 'warning' | 'danger' {
    return s === 'suspend' ? 'danger' : 'warning';
  }
</script>

<section class="add">
  <Card>
    <h2 class="sub">Adicionar bloqueio</h2>
    <form onsubmit={onAdd} class="form">
      <label class="fld">
        <span>Domínio</span>
        <input
          type="text"
          bind:value={domainInput}
          placeholder="ex.: pravda.example"
          autocomplete="off"
          spellcheck="false"
          required
        />
      </label>
      <label class="fld">
        <span>Severidade</span>
        <select bind:value={severityInput}>
          <option value="silence">Silenciar — só quem já segue continua vendo</option>
          <option value="suspend">Suspender — corte total (inbox rejeita)</option>
        </select>
      </label>
      <label class="fld wide">
        <span>Motivo (opcional)</span>
        <input
          type="text"
          bind:value={reasonInput}
          placeholder="Aparece no audit log"
        />
      </label>
      <Button type="submit" variant="danger" loading={adding}>Adicionar</Button>
    </form>
  </Card>
</section>

<section class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card>
      <EmptyState icon="block" title="Erro" description={error} />
    </Card>
  {:else if rows.length === 0}
    <Card>
      <EmptyState
        icon="block"
        title="Nenhum domínio bloqueado"
        description="A instância federa livremente. Adicione um bloqueio acima quando precisar cortar spam ou instância abusiva."
      />
    </Card>
  {:else}
    <Card padding="none">
      <table class="tbl">
        <thead>
          <tr>
            <th>Domínio</th>
            <th>Severidade</th>
            <th>Motivo</th>
            <th>Quando</th>
            <th>Por</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r (r.domain)}
            <tr>
              <td class="mono">{r.domain}</td>
              <td>
                <Badge tone={severityTone(r.severity)} size="sm">
                  {severityLabel(r.severity)}
                </Badge>
              </td>
              <td class="reason">{r.reason ?? '—'}</td>
              <td class="muted t" title={formatDate(r.created_at)}>
                {formatDate(r.created_at)}
              </td>
              <td class="muted">
                {r.created_by_handle ? `@${r.created_by_handle}` : '—'}
              </td>
              <td class="a">
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => onRemove(r.domain)}
                  loading={removingBusy.has(r.domain)}
                >
                  Remover
                </Button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </Card>
  {/if}
</section>

<style>
  .sub {
    font-size: var(--fs-md);
    margin: 0 0 var(--sp-3);
  }
  .add {
    margin-bottom: var(--sp-6);
  }
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
  }
  .fld.wide {
    grid-column: 1 / -2;
  }
  .fld input,
  .fld select {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  @media (max-width: 900px) {
    .form {
      grid-template-columns: 1fr;
    }
    .fld.wide {
      grid-column: auto;
    }
  }
  .tbl {
    width: 100%;
    border-collapse: collapse;
  }
  .tbl th,
  .tbl td {
    padding: var(--sp-2) var(--sp-3);
    text-align: left;
    border-bottom: 1px solid var(--border-subtle);
    font-size: var(--fs-sm);
    vertical-align: top;
  }
  .tbl thead th {
    background: var(--surface-2);
    font-weight: var(--fw-semibold);
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, monospace;
  }
  .reason {
    max-width: 20rem;
    word-break: break-word;
  }
  .t {
    font-size: var(--fs-xs);
  }
  .a {
    text-align: right;
  }
</style>
