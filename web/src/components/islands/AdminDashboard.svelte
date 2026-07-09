<script lang="ts">
  // Painel de métricas da instância. Reusa o endpoint /admin/stats que já
  // existe (getAdminStats). Cada card é uma métrica agregada; o botão
  // 'Atualizar' força refresh.
  import { onMount } from 'svelte';
  import { getAdminStats, type AdminStatsDto } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let stats = $state<AdminStatsDto | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let denied = $state<null | 'anon' | 'not-admin'>(null);

  async function load() {
    loading = true;
    err = null;
    const res = await getAdminStats();
    loading = false;
    if (res.success && res.data) {
      stats = res.data;
    } else if (res.error?.code === 'http_401') {
      denied = 'anon';
    } else if (res.error?.code === 'http_403') {
      denied = 'not-admin';
    } else {
      err = res.error?.message ?? 'Falha ao carregar métricas.';
    }
  }

  onMount(load);

  function fmtNum(n: number | null | undefined): string {
    if (n === null || n === undefined) return '—';
    return new Intl.NumberFormat('pt-BR').format(n);
  }
</script>

{#if denied === 'anon'}
  <Card padding="none">
    <EmptyState
      icon="lock"
      title="Entre para acessar o admin"
      description="Você precisa estar autenticado na sua conta administradora."
    >
      {#snippet action()}
        <Button href={`/entrar?next=${encodeURIComponent('/admin/')}`} variant="primary">
          Entrar
        </Button>
      {/snippet}
    </EmptyState>
  </Card>
{:else if denied === 'not-admin'}
  <Card padding="none">
    <EmptyState
      icon="lock"
      title="Acesso restrito"
      description="Esta área é reservada a administradores da instância DemocraciaBR."
    />
  </Card>
{:else if loading}
  <div class="grid cards">
    {#each Array(6) as _}
      <Card><Skeleton height="4rem" /></Card>
    {/each}
  </div>
{:else if err}
  <Alert tone="danger">{err}</Alert>
{:else if stats}
  <div class="grid cards">
    <Card>
      <div class="metric">
        <span class="k">Cidadãos</span>
        <strong class="v">{fmtNum(stats.citizens)}</strong>
        <span class="s muted">{fmtNum(stats.actors_local)} públicos no fediverso</span>
      </div>
    </Card>
    <Card>
      <div class="metric">
        <span class="k">Atores remotos vistos</span>
        <strong class="v">{fmtNum(stats.actors_remote)}</strong>
        <span class="s muted">Perfis de outras instâncias</span>
      </div>
    </Card>
    <Card>
      <div class="metric">
        <span class="k">Publicações</span>
        <strong class="v">{fmtNum(stats.notes_total)}</strong>
        <span class="s muted">+{fmtNum(stats.notes_last_7d)} nos últimos 7 dias</span>
      </div>
    </Card>
    <Card>
      <div class="metric">
        <span class="k">Mandatos</span>
        <strong class="v">{fmtNum(stats.mandates)}</strong>
        <span class="s muted">Câmara + Senado + subnacionais</span>
      </div>
    </Card>
    <Card>
      <div class="metric">
        <span class="k">Propostas</span>
        <strong class="v">{fmtNum(stats.proposals)}</strong>
        <span class="s muted">Total publicado</span>
      </div>
    </Card>
    <Card>
      <div class="metric">
        <span class="k">Notificações não lidas</span>
        <strong class="v">{fmtNum(stats.notifications_unread)}</strong>
        <span class="s muted">Somando todos os cidadãos</span>
      </div>
    </Card>
  </div>
  <div class="foot">
    <Button variant="ghost" size="sm" onclick={load}>
      <Icon name="cw" size={14} /> Atualizar
    </Button>
  </div>
{/if}

<style>
  .grid {
    display: grid;
    gap: var(--sp-3);
  }
  .cards {
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  }
  .metric {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .metric .k {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
    font-weight: var(--fw-semibold);
  }
  .metric .v {
    font-size: 1.65rem;
    font-weight: 700;
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }
  .metric .s {
    font-size: var(--fs-sm);
  }
  .foot {
    margin-top: var(--sp-3);
    display: flex;
    justify-content: flex-end;
  }
</style>
