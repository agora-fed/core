<script lang="ts">
  // Campanha de convites aos gabinetes (0.34.0): funil de elegíveis, envio
  // em lotes controlados e acompanhamento por status. O envio SEMPRE parte
  // de um clique do admin — nada automático.
  import { onMount } from 'svelte';
  import {
    getInviteCampaignOverview,
    sendInviteCampaignBatch,
    listInviteCampaign,
    type InviteCampaignOverviewDto,
    type InviteBatchResultDto,
    type InviteCampaignRowDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Badge from '../ui/Badge.svelte';
  import Skeleton from '../ui/Skeleton.svelte';

  let overview = $state<InviteCampaignOverviewDto | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);

  // Filtros do lote. Default 'estadual': federal já foi 100% convidado; estadual
  // é o conjunto com e-mail real ainda por convidar. 'todas' varre tudo.
  const SPHERES = [
    { v: 'estadual', label: 'Estadual (dep. estaduais)' },
    { v: 'federal', label: 'Federal (câmara/senado)' },
    { v: 'municipal', label: 'Municipal (vereadores)' },
    { v: 'todas', label: 'Todas as esferas' },
  ];
  let sphere = $state('estadual');
  let uf = $state('');
  let party = $state('');
  let batchSize = $state(20);

  let sending = $state(false);
  let batchResult = $state<InviteBatchResultDto | null>(null);
  let msg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  type Tab = 'pending' | 'accepted' | 'expired';
  let tab = $state<Tab>('pending');
  let rows = $state<InviteCampaignRowDto[]>([]);
  let rowsLoading = $state(false);

  const filter = () => ({
    sphere,
    uf: uf.trim() || undefined,
    party: party.trim() || undefined,
  });

  const sphereLabel = () =>
    SPHERES.find((s) => s.v === sphere)?.label ?? sphere;

  async function refreshOverview() {
    loading = true;
    const res = await getInviteCampaignOverview(filter());
    loading = false;
    if (res.success && res.data) {
      overview = res.data;
      loadError = null;
    } else {
      loadError = res.error?.message ?? 'Falha ao carregar. Você é admin?';
    }
  }

  async function refreshRows() {
    rowsLoading = true;
    const res = await listInviteCampaign(tab);
    rowsLoading = false;
    rows = res.success && res.data ? res.data : [];
  }

  async function doSendBatch() {
    if (sending) return;
    const n = Math.max(1, Math.min(50, batchSize));
    if (
      !confirm(
        `Enviar convites REAIS para os próximos ${n} gabinetes elegíveis` +
          ` — ${sphereLabel()}` +
          `${uf.trim() ? ` (UF ${uf.trim().toUpperCase()})` : ''}` +
          `${party.trim() ? ` (${party.trim().toUpperCase()})` : ''}?`,
      )
    )
      return;
    sending = true;
    msg = null;
    batchResult = null;
    const res = await sendInviteCampaignBatch({ ...filter(), limit: n });
    sending = false;
    if (res.success && res.data) {
      batchResult = res.data;
      msg = {
        kind: res.data.failed === 0 ? 'ok' : 'error',
        text: `Lote concluído: ${res.data.sent} enviados, ${res.data.failed} falhas.`,
      };
      await Promise.all([refreshOverview(), refreshRows()]);
    } else {
      msg = {
        kind: 'error',
        text: res.error?.message ?? 'Falha ao enviar o lote.',
      };
    }
  }

  function selectTab(t: Tab) {
    tab = t;
    void refreshRows();
  }

  const fmtDate = (iso: string) =>
    new Date(iso).toLocaleString('pt-BR', {
      day: '2-digit',
      month: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    });

  onMount(() => {
    void refreshOverview();
    void refreshRows();
  });
</script>

<Card>
  <h2>Funil — {sphereLabel()}</h2>
  {#if loading}
    <Skeleton width="60%" />
  {:else if loadError}
    <Alert tone="danger">{loadError}</Alert>
  {:else if overview}
    <div class="funnel">
      <div class="stat">
        <strong>{overview.total}</strong><span>mandatos</span>
      </div>
      <div class="stat">
        <strong>{overview.with_email}</strong><span>com e-mail público</span>
      </div>
      <div class="stat ok">
        <strong>{overview.bound}</strong><span>já na plataforma</span>
      </div>
      <div class="stat warn">
        <strong>{overview.invite_pending}</strong><span>convite pendente</span>
      </div>
      <div class="stat ok">
        <strong>{overview.invite_accepted}</strong><span>convites aceitos</span>
      </div>
      <div class="stat muted-stat">
        <strong>{overview.invite_expired}</strong><span>expirados</span>
      </div>
      <div class="stat accent">
        <strong>{overview.eligible_now}</strong><span>elegíveis agora</span>
      </div>
    </div>
  {/if}
</Card>

<Card>
  <h2>Enviar lote</h2>
  <p class="muted small">
    Convida os próximos elegíveis (nunca convidados ou com convite expirado),
    em ordem de UF e nome. Cada convite é um link único com prazo, enviado pro
    e-mail público oficial do mandato com o template
    <code>mandate_invite</code> (editável em Templates de e-mail).
  </p>
  <div class="batch-form">
    <label>
      <span>Esfera</span>
      <select class="input" bind:value={sphere} onchange={refreshOverview}>
        {#each SPHERES as s (s.v)}<option value={s.v}>{s.label}</option>{/each}
      </select>
    </label>
    <label>
      <span>UF (opcional)</span>
      <input class="input" maxlength="2" placeholder="ex.: SP" bind:value={uf} />
    </label>
    <label>
      <span>Partido (opcional)</span>
      <input class="input" placeholder="ex.: PT" bind:value={party} />
    </label>
    <label>
      <span>Tamanho do lote</span>
      <input class="input" type="number" min="1" max="50" bind:value={batchSize} />
    </label>
    <Button variant="primary" onclick={doSendBatch} loading={sending}>
      📨 Enviar lote
    </Button>
  </div>
  {#if msg}
    <div class="alert-slot">
      <Alert tone={msg.kind === 'ok' ? 'success' : 'danger'}>{msg.text}</Alert>
    </div>
  {/if}
  {#if batchResult}
    <ul class="batch-list">
      {#each batchResult.items as it}
        <li>
          {#if it.ok}✅{:else}❌{/if}
          <strong>{it.display_name}</strong>
          <span class="muted small">{it.email}</span>
          {#if it.error}<span class="err small">{it.error}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</Card>

<Card>
  <div class="tabs-row">
    <h2>Acompanhamento</h2>
    <div class="tabs">
      <button
        type="button"
        class="tab"
        class:active={tab === 'pending'}
        onclick={() => selectTab('pending')}>Pendentes</button
      >
      <button
        type="button"
        class="tab"
        class:active={tab === 'accepted'}
        onclick={() => selectTab('accepted')}>Aceitos</button
      >
      <button
        type="button"
        class="tab"
        class:active={tab === 'expired'}
        onclick={() => selectTab('expired')}>Expirados</button
      >
    </div>
  </div>
  {#if rowsLoading}
    <Skeleton width="80%" />
  {:else if rows.length === 0}
    <p class="muted">Nenhum convite {tab === 'pending' ? 'pendente' : tab === 'accepted' ? 'aceito' : 'expirado'}.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Parlamentar</th>
            <th>Partido/UF</th>
            <th>E-mail</th>
            <th>Enviado</th>
            <th>{tab === 'accepted' ? 'Aceito' : 'Expira'}</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r}
            <tr>
              <td>
                <a href={`/politicos/?id=${r.mandate_id}`}>{r.display_name}</a>
                <span class="muted small">{r.office}</span>
              </td>
              <td>
                <Badge>{r.party ?? '—'}/{r.uf ?? '—'}</Badge>
              </td>
              <td class="small">{r.email}</td>
              <td class="small">{fmtDate(r.sent_at)}</td>
              <td class="small">
                {fmtDate(tab === 'accepted' && r.accepted_at ? r.accepted_at : r.expires_at)}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</Card>

<style>
  h2 {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-lg);
  }
  .funnel {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
  }
  .stat {
    display: flex;
    flex-direction: column;
    min-width: 108px;
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    background: var(--surface-2);
  }
  .stat strong {
    font-size: var(--fs-xl);
    font-variant-numeric: tabular-nums;
  }
  .stat span {
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .stat.ok strong { color: var(--success, #15803d); }
  .stat.warn strong { color: var(--warning, #b45309); }
  .stat.accent {
    background: var(--accent-soft);
  }
  .stat.accent strong { color: var(--accent-strong); }
  .batch-form {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
    align-items: end;
  }
  .batch-form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: var(--fs-sm);
  }
  .batch-form .input {
    max-width: 130px;
  }
  .alert-slot { margin-top: var(--sp-3); }
  .batch-list {
    list-style: none;
    margin: var(--sp-3) 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .err { color: var(--danger, #b91c1c); }
  .tabs-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .tabs {
    display: flex;
    gap: 4px;
  }
  .tab {
    border: 1px solid var(--border, #e2e8f0);
    background: transparent;
    border-radius: var(--r-sm);
    padding: 4px 12px;
    cursor: pointer;
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .tab.active {
    background: var(--accent-soft);
    color: var(--accent-strong);
    border-color: transparent;
    font-weight: 600;
  }
  .table-wrap {
    overflow-x: auto;
    margin-top: var(--sp-3);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  th, td {
    text-align: left;
    padding: var(--sp-2);
    border-bottom: 1px solid var(--border, #e2e8f0);
    vertical-align: top;
  }
  td .muted.small { display: block; }
</style>
