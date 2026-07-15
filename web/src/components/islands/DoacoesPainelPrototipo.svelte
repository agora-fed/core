<script lang="ts">
  // PROTÓTIPO navegável do painel de gerenciamento de doações/financiamento de
  // campanha (serviço /servicos). Usa o perfil REAL da sessão (GET /api/v1/me)
  // pra pessoa se ver na interface; todo o resto é dado de exemplo em estado
  // local — nenhuma interação grava nada no servidor.
  import { onMount } from 'svelte';
  import { getMyProfile, type ProfileDto } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Badge from '../ui/Badge.svelte';
  import Tabs from '../ui/Tabs.svelte';
  import Icon from '../ui/Icon.svelte';
  import Input from '../ui/Input.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let ready = $state(false);
  let profile = $state<ProfileDto | null>(null);
  let active = $state('resumo');

  const brl = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });

  // ---- Dados de EXEMPLO (o serviço real vai persistir isso no backend) ----
  type Entrada = { data: string; origem: string; valor: number };
  type Saida = { data: string; categoria: string; valor: number };
  type Doacao = { nome: string; cpf: string; valor: number; recibo: string; data: string };

  let meta = $state(50_000);
  let entradas = $state<Entrada[]>([
    { data: '2026-07-10', origem: 'Doação — pessoa física', valor: 250 },
    { data: '2026-07-08', origem: 'Recursos próprios', valor: 5_000 },
    { data: '2026-07-05', origem: 'Fundo partidário', valor: 4_800 },
    { data: '2026-07-02', origem: 'Doação — financiamento coletivo', valor: 2_400 },
  ]);
  let saidas = $state<Saida[]>([
    { data: '2026-07-11', categoria: 'Material gráfico', valor: 1_850 },
    { data: '2026-07-09', categoria: 'Impulsionamento (registrado)', valor: 900 },
    { data: '2026-07-04', categoria: 'Deslocamento', valor: 320 },
  ]);
  let doacoes = $state<Doacao[]>([
    { nome: 'Maria S.', cpf: '***.***.123-45', valor: 250, recibo: 'RE-2026-0047', data: '2026-07-10' },
    { nome: 'João P.', cpf: '***.***.987-01', valor: 100, recibo: 'RE-2026-0046', data: '2026-07-09' },
    { nome: 'Ana L.', cpf: '***.***.456-78', valor: 500, recibo: 'RE-2026-0045', data: '2026-07-07' },
  ]);

  const totalEntradas = $derived(entradas.reduce((s, e) => s + e.valor, 0));
  const totalSaidas = $derived(saidas.reduce((s, e) => s + e.valor, 0));
  const pctMeta = $derived(Math.min(100, Math.round((totalEntradas / meta) * 100)));

  // Form local de "adicionar entrada" — só demonstra a interação.
  let novaOrigem = $state('');
  let novoValor = $state('');
  function addEntrada(e: SubmitEvent) {
    e.preventDefault();
    const valor = Number(novoValor.replace(',', '.'));
    if (!novaOrigem.trim() || !Number.isFinite(valor) || valor <= 0) {
      toast.error('Preencha origem e um valor positivo.');
      return;
    }
    entradas = [
      { data: new Date().toISOString().slice(0, 10), origem: novaOrigem.trim(), valor },
      ...entradas,
    ];
    novaOrigem = '';
    novoValor = '';
    toast.success('Protótipo: entrada adicionada só na tela — nada foi gravado.');
  }

  const tabs = $derived([
    { id: 'resumo', label: 'Visão geral' },
    { id: 'financiamento', label: 'Financiamento' },
    { id: 'doacoes', label: 'Doações', count: doacoes.length },
    { id: 'config', label: 'Configurações' },
  ]);

  function fmtData(iso: string) {
    const [y, m, d] = iso.split('-');
    return `${d}/${m}/${y}`;
  }

  onMount(async () => {
    const res = await getMyProfile();
    if (res.success && res.data) profile = res.data;
    ready = true;
  });
</script>

{#if !ready}
  <Card><Skeleton lines={4} /></Card>
{:else if !profile}
  <Card padding="none">
    <EmptyState
      icon="lock"
      title="Entre para testar o protótipo"
      description="O painel usa o seu perfil real pra você se ver na interface. Nada é gravado."
      action={loginAction}
    />
    {#snippet loginAction()}
      <Button href="/entrar" variant="primary">Entrar</Button>
    {/snippet}
  </Card>
{:else}
  <div class="proto-banner" role="note">
    <Icon name="info" size={16} />
    <span>
      <strong>Protótipo.</strong> Interface de demonstração com dados de exemplo —
      nenhuma ação aqui é salva. O serviço real vem depois da sua aprovação.
    </span>
  </div>

  <header class="head">
    <Avatar src={profile.avatar_url} name={profile.display_name ?? profile.public_handle} size="lg" />
    <div class="who">
      <h1>{profile.display_name ?? profile.public_handle}</h1>
      <p class="muted">
        @{profile.public_handle} · Candidatura nº <code>4501</code> (exemplo)
        <Badge tone="warning" size="sm">protótipo</Badge>
      </p>
    </div>
  </header>

  <Tabs {tabs} bind:active />

  {#if active === 'resumo'}
    <div class="stats">
      <Card>
        <p class="stat-lbl">Arrecadado</p>
        <p class="stat-val">{brl.format(totalEntradas)}</p>
        <div class="bar"><span style={`width:${pctMeta}%`}></span></div>
        <p class="muted stat-sub">{pctMeta}% da meta de {brl.format(meta)}</p>
      </Card>
      <Card>
        <p class="stat-lbl">Gasto declarado</p>
        <p class="stat-val">{brl.format(totalSaidas)}</p>
        <p class="muted stat-sub">{saidas.length} lançamentos</p>
      </Card>
      <Card>
        <p class="stat-lbl">Doações</p>
        <p class="stat-val">{doacoes.length}</p>
        <p class="muted stat-sub">todas com recibo eleitoral</p>
      </Card>
      <Card>
        <p class="stat-lbl">Transparência</p>
        <p class="stat-val ok"><Icon name="check" size={20} /> em dia</p>
        <p class="muted stat-sub">declaração pública atualizada</p>
      </Card>
    </div>
    <Card>
      <div class="pub-row">
        <div>
          <strong>Sua página pública</strong>
          <p class="muted" style="margin:2px 0 0">
            É o que o eleitor vê: entradas, saídas e histórico, ao lado do seu perfil.
          </p>
        </div>
        <Button variant="ghost" onclick={() => toast.info('Protótipo: a página pública vem na próxima fase.')}>
          Ver como eleitor
        </Button>
      </div>
    </Card>
  {:else if active === 'financiamento'}
    <Card>
      <h2>Adicionar entrada</h2>
      <form class="add-form" onsubmit={addEntrada}>
        <Input label="Origem" placeholder="Doação — pessoa física" bind:value={novaOrigem} />
        <Input label="Valor (R$)" placeholder="250,00" inputmode="decimal" bind:value={novoValor} />
        <Button type="submit" variant="primary">Adicionar</Button>
      </form>
      <p class="muted hint-proto">Protótipo: o lançamento aparece na lista abaixo, mas não é gravado.</p>
    </Card>
    <Card padding="none">
      <h2 class="tbl-title">Entradas — {brl.format(totalEntradas)}</h2>
      <div class="tbl-wrap">
        <table>
          <thead><tr><th>Data</th><th>Origem</th><th class="num">Valor</th></tr></thead>
          <tbody>
            {#each entradas as e (e.data + e.origem + e.valor)}
              <tr><td>{fmtData(e.data)}</td><td>{e.origem}</td><td class="num">{brl.format(e.valor)}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </Card>
    <Card padding="none">
      <h2 class="tbl-title">Saídas — {brl.format(totalSaidas)}</h2>
      <div class="tbl-wrap">
        <table>
          <thead><tr><th>Data</th><th>Categoria</th><th class="num">Valor</th></tr></thead>
          <tbody>
            {#each saidas as s (s.data + s.categoria)}
              <tr><td>{fmtData(s.data)}</td><td>{s.categoria}</td><td class="num">{brl.format(s.valor)}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </Card>
  {:else if active === 'doacoes'}
    <Card padding="none">
      <h2 class="tbl-title">Doações recebidas</h2>
      <div class="tbl-wrap">
        <table>
          <thead><tr><th>Data</th><th>Doador(a)</th><th>CPF</th><th>Recibo</th><th class="num">Valor</th></tr></thead>
          <tbody>
            {#each doacoes as d (d.recibo)}
              <tr>
                <td>{fmtData(d.data)}</td>
                <td>{d.nome}</td>
                <td><code>{d.cpf}</code></td>
                <td><code>{d.recibo}</code></td>
                <td class="num">{brl.format(d.valor)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </Card>
    <p class="muted rules">
      Regras aplicadas automaticamente no serviço real: só pessoa física, limite de
      10% dos rendimentos do ano anterior por doador(a), recibo eleitoral em toda
      doação, e espelhamento na declaração pública.
    </p>
  {:else}
    <Card>
      <h2>Configurações da arrecadação</h2>
      <div class="cfg">
        <Input label="Meta de arrecadação (R$)" value={String(meta)} hint="Aparece na sua página pública como barra de progresso." />
        <Input label="Conta bancária de campanha" placeholder="Banco · agência · conta" hint="Obrigatória pela lei eleitoral — as doações caem nos meios oficiais." />
        <Input label="Financiamento coletivo homologado (link)" placeholder="https://…" hint="Só plataformas homologadas pelo TSE." />
      </div>
      <Button variant="primary" onclick={() => toast.info('Protótipo: configurações não são salvas nesta demonstração.')}>
        Salvar
      </Button>
    </Card>
  {/if}
{/if}

<style>
  .proto-banner {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    margin-bottom: var(--sp-4);
    background: var(--accent-soft);
    color: var(--text-1);
    border: 1px dashed var(--accent);
    border-radius: var(--r-base);
    font-size: var(--fs-sm);
  }
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
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
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
  .pub-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  h2 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
  }
  .tbl-title {
    padding: var(--sp-4) var(--sp-4) 0;
  }
  .add-form {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-end;
    flex-wrap: wrap;
  }
  .add-form :global(.field) {
    flex: 1 1 180px;
    min-width: 0;
    margin-bottom: 0;
  }
  .hint-proto {
    margin: var(--sp-2) 0 0;
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
  .rules {
    font-size: var(--fs-sm);
    margin: var(--sp-3) 0 0;
  }
  .cfg {
    display: grid;
    gap: var(--sp-1);
    margin-bottom: var(--sp-3);
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
