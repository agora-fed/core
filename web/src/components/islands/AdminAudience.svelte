<script lang="ts">
  // Base de contatos (0.35.0): stats, listagem filtrável, import de listas
  // (colar CSV simples: email,nome,uf — uma linha por contato) e export.
  import { onMount } from 'svelte';
  import {
    getAudienceStats,
    listAudience,
    importAudience,
    deleteAudienceContact,
    AUDIENCE_EXPORT_URL,
    type AudienceStatsDto,
    type AudienceContactDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Badge from '../ui/Badge.svelte';
  import Skeleton from '../ui/Skeleton.svelte';

  let stats = $state<AudienceStatsDto | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);

  let status = $state<'active' | 'unsubscribed' | 'all'>('active');
  let q = $state('');
  let rows = $state<AudienceContactDto[]>([]);
  let rowsLoading = $state(false);

  // Import
  let importText = $state('');
  let importSlug = $state('');
  let importSegment = $state('cidadao');
  let importBasis = $state<'consent' | 'legitimate_interest'>('legitimate_interest');
  let importNotes = $state('');
  let importing = $state(false);
  let msg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function refreshStats() {
    loading = true;
    const res = await getAudienceStats();
    loading = false;
    if (res.success && res.data) {
      stats = res.data;
      loadError = null;
    } else {
      loadError = res.error?.message ?? 'Falha ao carregar. Você é admin?';
    }
  }

  async function refreshRows() {
    rowsLoading = true;
    const res = await listAudience({ status, q: q.trim() || undefined, limit: 200 });
    rowsLoading = false;
    rows = res.success && res.data ? res.data : [];
  }

  function parseImport(text: string) {
    return text
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l.length > 0 && !l.toLowerCase().startsWith('email'))
      .map((l) => {
        const [email, name, uf] = l.split(/[,;\t]/).map((p) => p?.trim() ?? '');
        return { email, name: name || undefined, uf: uf || undefined };
      })
      .filter((c) => c.email.includes('@'));
  }

  async function doImport() {
    if (importing) return;
    const contacts = parseImport(importText);
    if (contacts.length === 0) {
      msg = { kind: 'error', text: 'Nenhum e-mail válido no texto colado.' };
      return;
    }
    if (!importSlug.trim()) {
      msg = { kind: 'error', text: 'Dê um nome (slug) pra lista — ex.: evento-sp-2026.' };
      return;
    }
    if (
      !confirm(
        `Importar ${contacts.length} contatos como "${importSlug.trim()}" ` +
          `(base legal: ${importBasis === 'consent' ? 'consentimento' : 'legítimo interesse'})?`,
      )
    )
      return;
    importing = true;
    msg = null;
    const res = await importAudience({
      source_slug: importSlug.trim(),
      legal_basis: importBasis,
      segment: importSegment.trim() || undefined,
      notes: importNotes.trim() || undefined,
      contacts,
    });
    importing = false;
    if (res.success && res.data) {
      msg = {
        kind: 'ok',
        text: `Importados: ${res.data.upserted} (inválidos: ${res.data.invalid}).`,
      };
      importText = '';
      await Promise.all([refreshStats(), refreshRows()]);
    } else {
      msg = { kind: 'error', text: res.error?.message ?? 'Falha na importação.' };
    }
  }

  async function doDelete(c: AudienceContactDto) {
    if (!confirm(`Apagar DEFINITIVAMENTE ${c.email}? (uso: pedido LGPD)`)) return;
    const res = await deleteAudienceContact(c.id);
    if (res.success) {
      rows = rows.filter((r) => r.id !== c.id);
      await refreshStats();
    }
  }

  const fmtDate = (iso: string) =>
    new Date(iso).toLocaleDateString('pt-BR', { day: '2-digit', month: '2-digit', year: '2-digit' });

  onMount(() => {
    void refreshStats();
    void refreshRows();
  });
</script>

<Card>
  <h2>A base</h2>
  {#if loading}
    <Skeleton width="60%" />
  {:else if loadError}
    <Alert tone="danger">{loadError}</Alert>
  {:else if stats}
    <div class="funnel">
      <div class="stat accent"><strong>{stats.active}</strong><span>ativos</span></div>
      <div class="stat"><strong>{stats.total}</strong><span>total</span></div>
      <div class="stat"><strong>{stats.from_site}</strong><span>captados no site</span></div>
      <div class="stat"><strong>{stats.imported}</strong><span>importados</span></div>
      <div class="stat muted-stat"><strong>{stats.unsubscribed}</strong><span>descadastrados</span></div>
      {#each stats.segments as s}
        <div class="stat"><strong>{s.active}</strong><span>{s.segment}</span></div>
      {/each}
    </div>
    <div class="export-row">
      <a class="export" href={AUDIENCE_EXPORT_URL}>⬇️ Exportar CSV</a>
    </div>
  {/if}
</Card>

<Card>
  <h2>Importar lista</h2>
  <p class="muted small">
    Cole uma linha por contato: <code>email, nome, UF</code> (nome e UF
    opcionais; separador vírgula, ponto-e-vírgula ou tab). Quem já está na
    base não é duplicado; quem se descadastrou <strong>continua
    descadastrado</strong>.
  </p>
  <textarea
    class="input paste"
    rows="6"
    placeholder={'ana@exemplo.br, Ana Silva, SP\njoao@exemplo.br'}
    bind:value={importText}
  ></textarea>
  <div class="import-form">
    <label>
      <span>Nome da lista (slug)</span>
      <input class="input" placeholder="evento-sp-2026" bind:value={importSlug} />
    </label>
    <label>
      <span>Segmento</span>
      <input class="input" placeholder="cidadao" bind:value={importSegment} />
    </label>
    <label>
      <span>Base legal (LGPD)</span>
      <select class="input" bind:value={importBasis}>
        <option value="legitimate_interest">Legítimo interesse</option>
        <option value="consent">Consentimento (a lista foi coletada pra isso)</option>
      </select>
    </label>
    <label class="grow">
      <span>Origem/observações</span>
      <input
        class="input"
        placeholder="ex.: inscritos do evento X, coletados em jun/2026"
        bind:value={importNotes}
      />
    </label>
    <Button variant="primary" onclick={doImport} loading={importing}>
      📥 Importar
    </Button>
  </div>
  {#if msg}
    <div class="alert-slot">
      <Alert tone={msg.kind === 'ok' ? 'success' : 'danger'}>{msg.text}</Alert>
    </div>
  {/if}
</Card>

<Card>
  <div class="list-head">
    <h2>Contatos</h2>
    <div class="filters">
      <select class="input" bind:value={status} onchange={refreshRows}>
        <option value="active">Ativos</option>
        <option value="unsubscribed">Descadastrados</option>
        <option value="all">Todos</option>
      </select>
      <input
        class="input"
        placeholder="buscar e-mail/nome…"
        bind:value={q}
        onkeydown={(e) => e.key === 'Enter' && refreshRows()}
      />
      <Button variant="ghost" onclick={refreshRows}>Buscar</Button>
    </div>
  </div>
  {#if rowsLoading}
    <Skeleton width="80%" />
  {:else if rows.length === 0}
    <p class="muted">Nenhum contato — a base começa agora: divulgue o form do rodapé.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>E-mail</th>
            <th>Nome</th>
            <th>UF</th>
            <th>Segmento</th>
            <th>Origem</th>
            <th>Desde</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r}
            <tr class:unsub={r.unsubscribed}>
              <td>{r.email}</td>
              <td>{r.name ?? '—'}</td>
              <td>{r.uf ?? '—'}</td>
              <td><Badge>{r.segment}</Badge></td>
              <td class="small">{r.source}{r.unsubscribed ? ' · descadastrado' : ''}</td>
              <td class="small">{fmtDate(r.created_at)}</td>
              <td>
                <button class="del" title="Apagar (LGPD)" onclick={() => doDelete(r)}>🗑</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</Card>

<style>
  h2 { margin: 0 0 var(--sp-3); font-size: var(--fs-lg); }
  .funnel { display: flex; flex-wrap: wrap; gap: var(--sp-3); }
  .stat {
    display: flex; flex-direction: column; min-width: 96px;
    padding: var(--sp-2) var(--sp-3); border-radius: var(--r-sm);
    background: var(--surface-2);
  }
  .stat strong { font-size: var(--fs-xl); font-variant-numeric: tabular-nums; }
  .stat span { font-size: var(--fs-xs); color: var(--text-2); }
  .stat.accent { background: var(--accent-soft); }
  .stat.accent strong { color: var(--accent-strong); }
  .export-row { margin-top: var(--sp-3); }
  .export { color: var(--accent-strong); font-weight: 600; text-decoration: none; }
  .paste { width: 100%; font-family: var(--font-mono, monospace); font-size: var(--fs-sm); }
  .import-form {
    display: flex; flex-wrap: wrap; gap: var(--sp-3);
    align-items: end; margin-top: var(--sp-3);
  }
  .import-form label { display: flex; flex-direction: column; gap: 4px; font-size: var(--fs-sm); }
  .import-form label.grow { flex: 1; min-width: 220px; }
  .alert-slot { margin-top: var(--sp-3); }
  .list-head {
    display: flex; justify-content: space-between; align-items: center;
    gap: var(--sp-3); flex-wrap: wrap;
  }
  .filters { display: flex; gap: var(--sp-2); align-items: center; }
  .table-wrap { overflow-x: auto; margin-top: var(--sp-3); }
  table { width: 100%; border-collapse: collapse; font-size: var(--fs-sm); }
  th, td {
    text-align: left; padding: var(--sp-2);
    border-bottom: 1px solid var(--border, #e2e8f0);
  }
  tr.unsub td { opacity: 0.5; }
  .del { border: none; background: none; cursor: pointer; opacity: 0.6; }
  .del:hover { opacity: 1; }
</style>
