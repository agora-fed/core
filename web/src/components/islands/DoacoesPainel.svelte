<script lang="ts">
  // Painel de gerenciamento de doações/financiamento de campanha (0.31).
  // Exclusivo de contas "tipo político" (vínculo em mandate_identity_binding —
  // o servidor devolve is_politico e gateia toda escrita). Histórico imutável:
  // lançamento não se edita — revoga-se e lança-se de novo.
  import { onMount } from 'svelte';
  import {
    getMyProfile,
    getMinhaCampanha,
    addCampanhaLancamento,
    revokeCampanhaLancamento,
    saveCampanhaConfig,
    isAuthError,
    type ProfileDto,
    type CampanhaDto,
    type CampanhaEntryDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Tabs from '../ui/Tabs.svelte';
  import Icon from '../ui/Icon.svelte';
  import Input from '../ui/Input.svelte';
  import Switch from '../ui/Switch.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let ready = $state(false);
  let loggedOut = $state(false);
  let profile = $state<ProfileDto | null>(null);
  let campanha = $state<CampanhaDto | null>(null);
  let active = $state('resumo');

  const brl = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });

  const lancamentos = $derived(campanha?.lancamentos ?? []);
  const entradas = $derived(lancamentos.filter((l) => l.kind === 'entrada'));
  const saidas = $derived(lancamentos.filter((l) => l.kind === 'saida'));
  const doacoes = $derived(entradas.filter((l) => l.receipt_ref));
  const totalEntradas = $derived(entradas.reduce((s, e) => s + e.valor_centavos, 0));
  const totalSaidas = $derived(saidas.reduce((s, e) => s + e.valor_centavos, 0));
  const meta = $derived(campanha?.config?.meta_centavos ?? null);
  const pctMeta = $derived(
    meta && meta > 0 ? Math.min(100, Math.round((totalEntradas / meta) * 100)) : null,
  );

  const tabs = $derived([
    { id: 'resumo', label: 'Visão geral' },
    { id: 'financiamento', label: 'Financiamento' },
    { id: 'doacoes', label: 'Doações', count: doacoes.length },
    { id: 'config', label: 'Configurações' },
  ]);

  // ---- Form de lançamento --------------------------------------------------
  let fKind = $state<'entrada' | 'saida'>('entrada');
  let fDescricao = $state('');
  let fValor = $state('');
  let fData = $state('');
  let fRecibo = $state('');
  let fDoador = $state('');
  let saving = $state(false);

  /** "1.234,56" → 123456 centavos; null se inválido. */
  function parseReais(v: string): number | null {
    const n = Number(v.trim().replace(/\./g, '').replace(',', '.'));
    if (!Number.isFinite(n) || n <= 0) return null;
    return Math.round(n * 100);
  }

  async function submitLancamento(e: SubmitEvent) {
    e.preventDefault();
    if (saving) return;
    const valor = parseReais(fValor);
    if (!fDescricao.trim() || !valor || !fData) {
      toast.error('Preencha descrição, valor positivo e data.');
      return;
    }
    saving = true;
    const res = await addCampanhaLancamento({
      kind: fKind,
      descricao: fDescricao.trim(),
      valor_centavos: valor,
      occurred_on: fData,
      ...(fKind === 'entrada' && fRecibo.trim() ? { receipt_ref: fRecibo.trim() } : {}),
      ...(fKind === 'entrada' && fDoador.trim() ? { donor_name: fDoador.trim() } : {}),
    });
    saving = false;
    if (res.success && res.data && campanha) {
      const novo: CampanhaEntryDto = {
        id: res.data.id,
        kind: fKind,
        descricao: fDescricao.trim(),
        valor_centavos: valor,
        occurred_on: fData,
        receipt_ref: fKind === 'entrada' && fRecibo.trim() ? fRecibo.trim() : null,
        donor_name: fKind === 'entrada' && fDoador.trim() ? fDoador.trim() : null,
        created_at: new Date().toISOString(),
      };
      campanha = { ...campanha, lancamentos: [novo, ...campanha.lancamentos] };
      fDescricao = '';
      fValor = '';
      fRecibo = '';
      fDoador = '';
      toast.success('Lançamento publicado na sua declaração.');
    } else {
      toast.error(res.error?.message ?? 'Não foi possível lançar.');
    }
  }

  let revoking = $state<Set<string>>(new Set());
  async function revogar(l: CampanhaEntryDto) {
    if (revoking.has(l.id) || !campanha) return;
    if (!window.confirm(`Revogar "${l.descricao}" (${brl.format(l.valor_centavos / 100)})? A revogação fica registrada no histórico.`)) {
      return;
    }
    revoking = new Set(revoking).add(l.id);
    const res = await revokeCampanhaLancamento(l.id);
    const next = new Set(revoking);
    next.delete(l.id);
    revoking = next;
    if (res.success) {
      campanha = {
        ...campanha,
        lancamentos: campanha.lancamentos.filter((x) => x.id !== l.id),
      };
      toast.success('Lançamento revogado.');
    } else {
      toast.error(res.error?.message ?? 'Não foi possível revogar.');
    }
  }

  // ---- Configurações ---------------------------------------------------------
  let cMeta = $state('');
  let cBank = $state('');
  let cUrl = $state('');
  let cPublished = $state(false);
  let savingConfig = $state(false);

  async function submitConfig() {
    if (savingConfig) return;
    let metaCent: number | null = null;
    if (cMeta.trim()) {
      metaCent = parseReais(cMeta);
      if (!metaCent) {
        toast.error('Meta inválida — use um valor em reais, ex.: 50.000,00.');
        return;
      }
    }
    if (cUrl.trim() && !cUrl.trim().startsWith('https://')) {
      toast.error('O link de financiamento coletivo precisa ser https://.');
      return;
    }
    savingConfig = true;
    const res = await saveCampanhaConfig({
      meta_centavos: metaCent,
      bank_account: cBank.trim() || null,
      crowdfunding_url: cUrl.trim() || null,
      is_published: cPublished,
    });
    savingConfig = false;
    if (res.success) {
      if (campanha) {
        campanha = {
          ...campanha,
          config: {
            meta_centavos: metaCent,
            bank_account: cBank.trim() || null,
            crowdfunding_url: cUrl.trim() || null,
            is_published: cPublished,
          },
        };
      }
      toast.success('Configurações salvas.');
    } else {
      toast.error(res.error?.message ?? 'Não foi possível salvar.');
    }
  }

  function fmtData(iso: string) {
    const [y, m, d] = iso.split('-');
    return `${d}/${m}/${y}`;
  }

  // isAuthError só reconhece respostas SEM envelope (http_401/403); os
  // handlers devolvem envelope com code "unauthorized" — cobre os dois.
  function isLoggedOut<T>(res: import('../../lib/types').ApiResponse<T>) {
    return !res.success && (isAuthError(res) || res.error?.code === 'unauthorized');
  }

  onMount(async () => {
    const [p, c] = await Promise.all([getMyProfile(), getMinhaCampanha()]);
    if (isLoggedOut(c) || isLoggedOut(p)) {
      loggedOut = true;
      ready = true;
      return;
    }
    if (p.success && p.data) profile = p.data;
    if (c.success && c.data) {
      campanha = c.data;
      cMeta = c.data.config?.meta_centavos
        ? (c.data.config.meta_centavos / 100).toLocaleString('pt-BR', { minimumFractionDigits: 2 })
        : '';
      cBank = c.data.config?.bank_account ?? '';
      cUrl = c.data.config?.crowdfunding_url ?? '';
      cPublished = c.data.config?.is_published ?? false;
    }
    fData = new Date().toISOString().slice(0, 10);
    ready = true;
  });
</script>

{#if !ready}
  <Card><Skeleton lines={4} /></Card>
{:else if loggedOut}
  <Card padding="none">
    <EmptyState
      icon="lock"
      title="Entre para gerenciar sua campanha"
      description="O painel de doações e financiamento exige sessão."
      action={loginAction}
    />
    {#snippet loginAction()}
      <Button href="/entrar" variant="primary">Entrar</Button>
    {/snippet}
  </Card>
{:else if !campanha?.is_politico}
  <Card padding="none">
    <EmptyState
      icon="lock"
      title="Serviço exclusivo de candidaturas e mandatos"
      description="O painel de doações abre para contas vinculadas a um mandato na plataforma. Se você opera um mandato, aceite o convite de vínculo; saiba mais na página de serviços."
      action={gateAction}
    />
    {#snippet gateAction()}
      <Button href="/servicos" variant="primary">Conhecer os serviços</Button>
    {/snippet}
  </Card>
{:else}
  <header class="head">
    <Avatar
      src={profile?.avatar_url ?? null}
      name={profile?.display_name ?? profile?.public_handle ?? '?'}
      size="lg"
    />
    <div class="who">
      <h1>{profile?.display_name ?? profile?.public_handle}</h1>
      <p class="muted">
        Doações e financiamento de campanha
        {#if campanha.config?.is_published}
          · <span class="pub-ok">página pública ativa</span>
        {:else}
          · <span class="pub-off">página pública desativada</span>
        {/if}
      </p>
    </div>
  </header>

  <Tabs {tabs} bind:active />

  {#if active === 'resumo'}
    <div class="stats">
      <Card>
        <p class="stat-lbl">Arrecadado</p>
        <p class="stat-val">{brl.format(totalEntradas / 100)}</p>
        {#if pctMeta !== null && meta}
          <div class="bar"><span style={`width:${pctMeta}%`}></span></div>
          <p class="muted stat-sub">{pctMeta}% da meta de {brl.format(meta / 100)}</p>
        {:else}
          <p class="muted stat-sub">defina uma meta em Configurações</p>
        {/if}
      </Card>
      <Card>
        <p class="stat-lbl">Gasto declarado</p>
        <p class="stat-val">{brl.format(totalSaidas / 100)}</p>
        <p class="muted stat-sub">{saidas.length} {saidas.length === 1 ? 'lançamento' : 'lançamentos'}</p>
      </Card>
      <Card>
        <p class="stat-lbl">Doações</p>
        <p class="stat-val">{doacoes.length}</p>
        <p class="muted stat-sub">com recibo eleitoral</p>
      </Card>
      <Card>
        <p class="stat-lbl">Declaração</p>
        <p class="stat-val ok">
          <Icon name="check" size={20} />
          {lancamentos.length}
        </p>
        <p class="muted stat-sub">lançamentos públicos ativos</p>
      </Card>
    </div>
    {#if lancamentos.length === 0}
      <Card padding="none">
        <EmptyState
          icon="feed"
          title="Sua declaração está vazia"
          description="Publique o primeiro lançamento na aba Financiamento — cada entrada e saída vira registro público."
        />
      </Card>
    {/if}
  {:else if active === 'financiamento'}
    <Card>
      <h2>Novo lançamento</h2>
      <form class="add-form" onsubmit={submitLancamento}>
        <div class="kind-row" role="radiogroup" aria-label="Tipo de lançamento">
          <label class="kind" class:on={fKind === 'entrada'}>
            <input type="radio" bind:group={fKind} value="entrada" /> Entrada
          </label>
          <label class="kind" class:on={fKind === 'saida'}>
            <input type="radio" bind:group={fKind} value="saida" /> Saída
          </label>
        </div>
        <div class="fields">
          <Input
            label={fKind === 'entrada' ? 'Origem' : 'Categoria do gasto'}
            placeholder={fKind === 'entrada' ? 'Doação — pessoa física' : 'Material gráfico'}
            bind:value={fDescricao}
            maxlength={200}
          />
          <Input label="Valor (R$)" placeholder="250,00" inputmode="decimal" bind:value={fValor} />
          <div class="field-date">
            <label for="lanc-data">Data</label>
            <input id="lanc-data" type="date" bind:value={fData} />
          </div>
          {#if fKind === 'entrada'}
            <Input
              label="Recibo eleitoral (se doação)"
              placeholder="RE-2026-0001"
              bind:value={fRecibo}
              maxlength={60}
              hint="Preencher o recibo marca o lançamento como doação."
            />
            <Input
              label="Doador(a) — nome público resumido"
              placeholder="Maria S."
              bind:value={fDoador}
              maxlength={120}
              hint="Nunca publique CPF ou dados de contato."
            />
          {/if}
        </div>
        <Button type="submit" variant="primary" disabled={saving} loading={saving}>
          Publicar lançamento
        </Button>
      </form>
      <p class="muted hint-imutavel">
        A declaração é imutável: lançamento publicado não se edita — revoga-se
        (fica no histórico) e publica-se o correto.
      </p>
    </Card>
    <Card padding="none">
      <h2 class="tbl-title">Entradas — {brl.format(totalEntradas / 100)}</h2>
      {#if entradas.length === 0}
        <p class="tbl-empty muted">Nenhuma entrada ainda.</p>
      {:else}
        <div class="tbl-wrap">
          <table>
            <thead><tr><th>Data</th><th>Origem</th><th class="num">Valor</th><th></th></tr></thead>
            <tbody>
              {#each entradas as l (l.id)}
                <tr>
                  <td>{fmtData(l.occurred_on)}</td>
                  <td>
                    {l.descricao}
                    {#if l.receipt_ref}<code class="rec">{l.receipt_ref}</code>{/if}
                  </td>
                  <td class="num">{brl.format(l.valor_centavos / 100)}</td>
                  <td class="act">
                    <button
                      type="button"
                      class="revoke"
                      title="Revogar lançamento"
                      disabled={revoking.has(l.id)}
                      onclick={() => revogar(l)}
                    >✕</button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>
    <Card padding="none">
      <h2 class="tbl-title">Saídas — {brl.format(totalSaidas / 100)}</h2>
      {#if saidas.length === 0}
        <p class="tbl-empty muted">Nenhuma saída ainda.</p>
      {:else}
        <div class="tbl-wrap">
          <table>
            <thead><tr><th>Data</th><th>Categoria</th><th class="num">Valor</th><th></th></tr></thead>
            <tbody>
              {#each saidas as l (l.id)}
                <tr>
                  <td>{fmtData(l.occurred_on)}</td>
                  <td>{l.descricao}</td>
                  <td class="num">{brl.format(l.valor_centavos / 100)}</td>
                  <td class="act">
                    <button
                      type="button"
                      class="revoke"
                      title="Revogar lançamento"
                      disabled={revoking.has(l.id)}
                      onclick={() => revogar(l)}
                    >✕</button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>
  {:else if active === 'doacoes'}
    <Card padding="none">
      <h2 class="tbl-title">Doações — entradas com recibo eleitoral</h2>
      {#if doacoes.length === 0}
        <p class="tbl-empty muted">
          Nenhuma doação registrada — lance uma entrada com o número do recibo
          eleitoral na aba Financiamento.
        </p>
      {:else}
        <div class="tbl-wrap">
          <table>
            <thead><tr><th>Data</th><th>Doador(a)</th><th>Recibo</th><th class="num">Valor</th></tr></thead>
            <tbody>
              {#each doacoes as d (d.id)}
                <tr>
                  <td>{fmtData(d.occurred_on)}</td>
                  <td>{d.donor_name ?? '—'}</td>
                  <td><code>{d.receipt_ref}</code></td>
                  <td class="num">{brl.format(d.valor_centavos / 100)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>
    <p class="muted rules">
      Lembrete das regras (Lei 9.504/1997): só pessoa física doa; limite de 10%
      dos rendimentos brutos do ano anterior por doador(a); toda doação exige
      recibo eleitoral e entra na prestação de contas oficial. A arrecadação em
      dinheiro acontece nos meios oficiais — aqui é a declaração pública.
    </p>
  {:else}
    <Card>
      <h2>Configurações da arrecadação</h2>
      <div class="cfg">
        <Input
          label="Meta de arrecadação (R$)"
          placeholder="50.000,00"
          bind:value={cMeta}
          inputmode="decimal"
          hint="Aparece como barra de progresso na visão geral e na página pública."
        />
        <Input
          label="Conta bancária de campanha"
          placeholder="Banco · agência · conta"
          bind:value={cBank}
          maxlength={200}
          hint="Obrigatória pela lei eleitoral — doações caem nos meios oficiais."
        />
        <Input
          label="Financiamento coletivo homologado (link)"
          placeholder="https://…"
          bind:value={cUrl}
          maxlength={300}
          hint="Só plataformas homologadas pelo TSE."
        />
        <div class="pub-toggle">
          <Switch bind:checked={cPublished} label="Publicar página de arrecadação" />
        </div>
      </div>
      <Button
        variant="primary"
        onclick={submitConfig}
        disabled={savingConfig}
        loading={savingConfig}
      >
        Salvar
      </Button>
    </Card>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-2xl);
  }
  .who p {
    margin: 2px 0 0;
  }
  .pub-ok {
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .pub-off {
    color: var(--text-3);
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: var(--sp-3);
    margin: var(--sp-4) 0;
  }
  .stat-lbl {
    margin: 0 0 2px;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  .stat-val {
    margin: 0;
    font-size: var(--fs-xl);
    font-weight: var(--fw-bold);
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }
  .stat-val.ok {
    color: var(--accent-strong);
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .stat-sub {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-xs);
  }
  .bar {
    height: 6px;
    background: var(--surface-2);
    border-radius: var(--r-full);
    overflow: hidden;
    margin-top: var(--sp-2);
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  h2 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
  }
  .tbl-title {
    padding: var(--sp-4) var(--sp-4) 0;
  }
  .tbl-empty {
    padding: var(--sp-3) var(--sp-4) var(--sp-4);
    margin: 0;
    font-size: var(--fs-sm);
  }
  .add-form {
    display: grid;
    gap: var(--sp-3);
  }
  .kind-row {
    display: flex;
    gap: var(--sp-2);
  }
  .kind {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-full);
    cursor: pointer;
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .kind.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .fields {
    display: grid;
    gap: var(--sp-1);
    grid-template-columns: 1fr;
  }
  @media (min-width: 640px) {
    .fields {
      grid-template-columns: 1fr 1fr;
    }
  }
  .fields :global(.field) {
    min-width: 0;
  }
  .field-date label {
    display: block;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-1);
    color: var(--text-1);
  }
  .field-date input {
    width: 100%;
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    color: var(--text-1);
    font: inherit;
  }
  .hint-imutavel {
    margin: var(--sp-3) 0 0;
    font-size: var(--fs-xs);
  }
  .tbl-wrap {
    overflow-x: auto;
    padding: var(--sp-2) 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  th,
  td {
    text-align: left;
    padding: var(--sp-2) var(--sp-4);
    border-top: 1px solid var(--border-subtle);
    color: var(--text-2);
    white-space: nowrap;
  }
  thead th {
    border-top: 0;
    color: var(--text-3);
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .act {
    width: 1%;
    text-align: right;
  }
  .revoke {
    border: 0;
    background: none;
    color: var(--text-3);
    cursor: pointer;
    font-size: var(--fs-sm);
    padding: var(--sp-1);
    border-radius: var(--r-sm);
  }
  .revoke:hover {
    color: var(--danger);
    background: var(--surface-2);
  }
  .rec {
    margin-left: var(--sp-2);
  }
  .rules {
    font-size: var(--fs-sm);
    margin: var(--sp-3) 0 0;
  }
  .cfg {
    display: grid;
    gap: var(--sp-1);
    margin-bottom: var(--sp-3);
  }
  .pub-toggle {
    margin: var(--sp-2) 0;
  }
  .muted {
    color: var(--text-3);
  }
  code {
    background: var(--surface-2);
    padding: 1px 5px;
    border-radius: var(--r-sm);
    font-size: 0.9em;
  }
  :global(.card + .card) {
    margin-top: var(--sp-3);
  }
</style>
