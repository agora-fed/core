<script lang="ts">
  // Audit log: leitura simples da tabela admin_audit via /admin/audit.
  // Uma linha por ação, mais recente primeiro. Sem paginação nessa fatia
  // (limit=100). O DTO já traz o handle do admin e do alvo hidratados.
  import { onMount } from 'svelte';
  import { adminListAudit, type AdminAuditRowDto } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let rows = $state<AdminAuditRowDto[]>([]);

  onMount(async () => {
    const res = await adminListAudit(100, 0);
    loading = false;
    if (res.success && res.data) {
      rows = res.data;
    } else {
      error = res.error?.message ?? 'Falha ao carregar auditoria.';
    }
  });

  function actionLabel(a: string): string {
    switch (a) {
      case 'account_suspend': return 'Suspendeu conta';
      case 'account_unsuspend': return 'Retirou suspensão';
      case 'account_silence': return 'Silenciou conta';
      case 'account_unsilence': return 'Retirou silenciamento';
      case 'account_role_change': return 'Alterou papel';
      case 'report_resolve': return 'Resolveu denúncia';
      case 'report_reopen': return 'Reabriu denúncia';
      case 'server_domain_block': return 'Bloqueou domínio (server)';
      case 'server_domain_unblock': return 'Removeu bloqueio de domínio';
      case 'note_hide': return 'Ocultou publicação';
      default: return a;
    }
  }

  function actionTone(a: string): 'info' | 'warning' | 'danger' | 'success' {
    if (a.includes('suspend') || a.includes('block') || a.includes('hide')) return 'danger';
    if (a.includes('silence')) return 'warning';
    if (a.includes('resolve') || a.includes('un')) return 'success';
    return 'info';
  }

  function targetLabel(r: AdminAuditRowDto): string {
    if (r.target_citizen_handle) return `@${r.target_citizen_handle}`;
    if (r.target_domain) return r.target_domain;
    if (r.target_id) return r.target_id.slice(0, 8) + '…';
    return '—';
  }

  function detailText(d: unknown): string {
    if (!d || typeof d !== 'object') return '';
    const obj = d as Record<string, unknown>;
    const bits: string[] = [];
    if (typeof obj.reason === 'string') bits.push(`Motivo: ${obj.reason}`);
    if (typeof obj.severity === 'string') bits.push(`Severidade: ${obj.severity}`);
    if (typeof obj.notes === 'string') bits.push(`Notas: ${obj.notes}`);
    return bits.join(' · ');
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <Card>
    <EmptyState icon="shield" title="Erro" description={error} />
  </Card>
{:else if rows.length === 0}
  <Card>
    <EmptyState
      icon="shield"
      title="Sem ações registradas"
      description="Quando um moderador tomar alguma ação (suspender, silenciar, bloquear domínio, resolver denúncia), ela aparece aqui."
    />
  </Card>
{:else}
  <Card padding="none">
    <table class="tbl">
      <thead>
        <tr>
          <th>Quando</th>
          <th>Admin</th>
          <th>Ação</th>
          <th>Alvo</th>
          <th>Detalhes</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as r (r.id)}
          <tr>
            <td class="muted t" title={formatDate(r.created_at)}>
              {formatDate(r.created_at)}
            </td>
            <td class="mono">
              {r.admin_handle ? `@${r.admin_handle}` : r.admin_id.slice(0, 8) + '…'}
            </td>
            <td>
              <Badge tone={actionTone(r.action)} size="sm">
                {actionLabel(r.action)}
              </Badge>
            </td>
            <td class="mono">{targetLabel(r)}</td>
            <td class="detail muted">{detailText(r.detail) || '—'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </Card>
{/if}

<style>
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
  .t {
    white-space: nowrap;
    font-size: var(--fs-xs);
  }
  .detail {
    max-width: 30rem;
    word-break: break-word;
  }
</style>
