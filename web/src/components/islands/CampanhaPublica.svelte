<script lang="ts">
  // Página PÚBLICA da declaração de financiamento de uma candidatura — o que
  // o eleitor vê quando a campanha publica (is_published). Lê ?u=<handle>,
  // igual /perfil. 404 do servidor vira um estado vazio educado.
  import { onMount } from 'svelte';
  import { getCampanhaPublica, type CampanhaPublicaDto } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let ready = $state(false);
  let data = $state<CampanhaPublicaDto | null>(null);
  let handle = $state('');

  const brl = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });

  const entradas = $derived((data?.lancamentos ?? []).filter((l) => l.kind === 'entrada'));
  const saidas = $derived((data?.lancamentos ?? []).filter((l) => l.kind === 'saida'));
  const doacoes = $derived(entradas.filter((l) => l.receipt_ref));
  const pctMeta = $derived(
    data?.meta_centavos && data.meta_centavos > 0
      ? Math.min(100, Math.round((data.total_entradas_centavos / data.meta_centavos) * 100))
      : null,
  );

  function fmtData(iso: string) {
    const [y, m, d] = iso.split('-');
    return `${d}/${m}/${y}`;
  }

  onMount(async () => {
    handle = new URLSearchParams(window.location.search).get('u') ?? '';
    if (handle) {
      const res = await getCampanhaPublica(handle);
      if (res.success && res.data) data = res.data;
    }
    ready = true;
  });
</script>

{#if !ready}
  <Card><Skeleton lines={4} /></Card>
{:else if !data}
  <Card padding="none">
    <EmptyState
      icon="search"
      title="Declaração de campanha não encontrada"
      description="Ou o endereço está errado, ou esta candidatura ainda não publicou a declaração de financiamento."
      action={backAction}
    />
    {#snippet backAction()}
      <Button href="/politicos" variant="primary">Ver os políticos</Button>
    {/snippet}
  </Card>
{:else}
  <header class="head">
    <Avatar src={data.avatar_url} name={data.display_name ?? data.handle} size="lg" />
    <div class="who">
      <h1>{data.display_name ?? data.handle}</h1>
      <p class="muted">
        <a href={`/perfil/?u=${encodeURIComponent(data.handle)}`}>@{data.handle}</a>
        · financiamento de campanha declarado publicamente
      </p>
    </div>
  </header>

  <div class="stats">
    <Card>
      <p class="stat-lbl">Arrecadado</p>
      <p class="stat-val">{brl.format(data.total_entradas_centavos / 100)}</p>
      {#if pctMeta !== null && data.meta_centavos}
        <div class="bar"><span style={`width:${pctMeta}%`}></span></div>
        <p class="muted stat-sub">
          {pctMeta}% da meta de {brl.format(data.meta_centavos / 100)}
        </p>
      {/if}
    </Card>
    <Card>
      <p class="stat-lbl">Gasto declarado</p>
      <p class="stat-val">{brl.format(data.total_saidas_centavos / 100)}</p>
      <p class="muted stat-sub">{saidas.length} {saidas.length === 1 ? 'lançamento' : 'lançamentos'}</p>
    </Card>
    <Card>
      <p class="stat-lbl">Doações</p>
      <p class="stat-val">{data.doacoes_count}</p>
      <p class="muted stat-sub">com recibo eleitoral</p>
    </Card>
  </div>

  {#if data.bank_account || data.crowdfunding_url}
    <Card>
      <h2><Icon name="check" size={16} /> Como doar (meios oficiais)</h2>
      <ul class="donate">
        {#if data.crowdfunding_url}
          <li>
            <a href={data.crowdfunding_url} rel="noopener noreferrer nofollow" target="_blank">
              Financiamento coletivo homologado pelo TSE ↗
            </a>
          </li>
        {/if}
        {#if data.bank_account}
          <li>Conta de campanha: <code>{data.bank_account}</code></li>
        {/if}
      </ul>
      <p class="muted legal">
        Só pessoa física pode doar, até 10% dos rendimentos brutos do ano
        anterior; toda doação exige recibo eleitoral (Lei 9.504/1997). A
        DemocraciaBR não intermedeia pagamento.
      </p>
    </Card>
  {/if}

  <Card padding="none">
    <h2 class="tbl-title">Entradas — {brl.format(data.total_entradas_centavos / 100)}</h2>
    {#if entradas.length === 0}
      <p class="tbl-empty muted">Nenhuma entrada declarada ainda.</p>
    {:else}
      <div class="tbl-wrap">
        <table>
          <thead><tr><th>Data</th><th>Origem</th><th>Doador(a)</th><th>Recibo</th><th class="num">Valor</th></tr></thead>
          <tbody>
            {#each entradas as l (l.id)}
              <tr>
                <td>{fmtData(l.occurred_on)}</td>
                <td>{l.descricao}</td>
                <td>{l.donor_name ?? '—'}</td>
                <td>{#if l.receipt_ref}<code>{l.receipt_ref}</code>{:else}—{/if}</td>
                <td class="num">{brl.format(l.valor_centavos / 100)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </Card>

  <Card padding="none">
    <h2 class="tbl-title">Saídas — {brl.format(data.total_saidas_centavos / 100)}</h2>
    {#if saidas.length === 0}
      <p class="tbl-empty muted">Nenhum gasto declarado ainda.</p>
    {:else}
      <div class="tbl-wrap">
        <table>
          <thead><tr><th>Data</th><th>Categoria</th><th class="num">Valor</th></tr></thead>
          <tbody>
            {#each saidas as l (l.id)}
              <tr>
                <td>{fmtData(l.occurred_on)}</td>
                <td>{l.descricao}</td>
                <td class="num">{brl.format(l.valor_centavos / 100)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </Card>

  <p class="muted foot">
    Declaração voluntária, com histórico imutável (correções acontecem por
    revogação registrada) — complementa, não substitui, a prestação de contas
    oficial no
    <a href="https://divulgacandcontas.tse.jus.br" rel="noopener noreferrer" target="_blank">
      DivulgaCandContas do TSE ↗</a
    >.
  </p>
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
  .who a {
    color: var(--accent-strong);
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: var(--sp-3);
    margin-bottom: var(--sp-3);
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
    display: flex;
    align-items: center;
    gap: var(--sp-2);
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
  .donate {
    list-style: none;
    padding: 0;
    margin: 0 0 var(--sp-2);
    display: grid;
    gap: var(--sp-1);
  }
  .donate a {
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .legal {
    margin: 0;
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
  .foot {
    margin: var(--sp-4) 0 0;
    font-size: var(--fs-sm);
  }
  .foot a {
    color: var(--accent-strong);
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
