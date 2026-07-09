<script lang="ts">
  // /convites — cria e lista convites do próprio cidadão. Link pra
  // compartilhar é montado no cliente (`/convite?token=…`), com botão
  // Copiar. Sem paginação (limit=200 no backend basta pro escopo).
  import { onMount } from 'svelte';
  import {
    listMyInvitations,
    createInvitation,
    deleteInvitation,
    type InvitationDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<InvitationDto[]>([]);
  let deletingBusy = $state<Set<string>>(new Set());
  let copiedToken = $state<string | null>(null);

  // Form
  let email = $state('');
  let notes = $state('');
  let maxUses = $state(1);
  let expiresChoice = $state<'1' | '24' | '168' | '720'>('168');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await listMyInvitations();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar convites.';
    }
  }
  onMount(reload);

  function inviteLink(token: string): string {
    if (typeof window === 'undefined') return `/convite?token=${token}`;
    return `${window.location.origin}/convite?token=${token}`;
  }

  async function onCreate(e: SubmitEvent) {
    e.preventDefault();
    creating = true;
    const res = await createInvitation({
      target_email: email.trim() || undefined,
      notes: notes.trim() || undefined,
      max_uses: maxUses,
      expires_in_hours: Number(expiresChoice),
    });
    creating = false;
    if (res.success && res.data) {
      items = [res.data, ...items];
      email = '';
      notes = '';
      maxUses = 1;
      expiresChoice = '168';
      toast.success('Convite criado.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao criar convite.');
    }
  }

  async function onDelete(id: string) {
    if (deletingBusy.has(id)) return;
    if (!confirm('Revogar este convite?')) return;
    deletingBusy = new Set(deletingBusy).add(id);
    const res = await deleteInvitation(id);
    const done = new Set(deletingBusy);
    done.delete(id);
    deletingBusy = done;
    if (res.success) {
      items = items.filter((i) => i.id !== id);
      toast.success('Convite revogado.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao revogar.');
    }
  }

  async function onCopy(token: string) {
    try {
      await navigator.clipboard.writeText(inviteLink(token));
      copiedToken = token;
      setTimeout(() => {
        if (copiedToken === token) copiedToken = null;
      }, 1800);
      toast.success('Link copiado.');
    } catch {
      toast.error('Copie manualmente: ' + inviteLink(token));
    }
  }

  function statusOf(i: InvitationDto): { label: string; tone: 'ok' | 'warn' | 'off' } {
    if (i.uses_left <= 0) return { label: 'Esgotado', tone: 'off' };
    if (i.expires_at && new Date(i.expires_at) < new Date()) return { label: 'Expirado', tone: 'off' };
    return { label: `${i.uses_left}/${i.max_uses} usos restantes`, tone: 'ok' };
  }
</script>

<Card>
  <h2 class="sub">Novo convite</h2>
  <form onsubmit={onCreate} class="form">
    <label class="fld">
      <span>E-mail alvo (opcional)</span>
      <input
        type="email"
        bind:value={email}
        placeholder="Se preencher, só esse e-mail pode usar"
      />
    </label>
    <label class="fld">
      <span>Notas (opcional, só você vê)</span>
      <input type="text" bind:value={notes} maxlength="200" placeholder="Ex.: colega da faculdade" />
    </label>
    <label class="fld">
      <span>Usos</span>
      <input type="number" bind:value={maxUses} min="1" max="1000" />
    </label>
    <label class="fld">
      <span>Validade</span>
      <select bind:value={expiresChoice}>
        <option value="1">1 hora</option>
        <option value="24">1 dia</option>
        <option value="168">7 dias</option>
        <option value="720">30 dias</option>
      </select>
    </label>
    <div class="fld btn-cell">
      <Button type="submit" variant="primary" loading={creating}>
        Gerar convite
      </Button>
    </div>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card>
      <EmptyState icon="link" title="Erro" description={error} />
    </Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="link"
        title="Sem convites"
        description="Você ainda não gerou nenhum. Use o formulário acima."
      />
    </Card>
  {:else}
    <ul class="rows">
      {#each items as inv (inv.id)}
        {@const st = statusOf(inv)}
        <li>
          <Card>
            <div class="row">
              <div class="info">
                <div class="link-line">
                  <code class="link">{inviteLink(inv.token)}</code>
                  <Button variant="ghost" size="sm" onclick={() => onCopy(inv.token)}>
                    {copiedToken === inv.token ? 'Copiado ✓' : 'Copiar'}
                  </Button>
                </div>
                <div class="meta muted">
                  <span class={`chip chip-${st.tone}`}>{st.label}</span>
                  {#if inv.target_email}
                    · pra <code>{inv.target_email}</code>
                  {/if}
                  · criado {formatDate(inv.created_at)}
                  {#if inv.expires_at}
                    · expira {formatDate(inv.expires_at)}
                  {/if}
                  {#if inv.notes}
                    · <em>{inv.notes}</em>
                  {/if}
                </div>
              </div>
              <Button
                variant="ghost"
                size="sm"
                onclick={() => onDelete(inv.id)}
                loading={deletingBusy.has(inv.id)}
              >
                Revogar
              </Button>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sub {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-md);
  }
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr;
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
  .btn-cell {
    grid-column: 1 / -1;
    align-items: flex-start;
  }
  @media (max-width: 700px) {
    .form {
      grid-template-columns: 1fr;
    }
  }
  .list {
    margin-top: var(--sp-4);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-start;
  }
  .info {
    flex: 1;
    min-width: 0;
  }
  .link-line {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .link {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-2);
    padding: 4px 8px;
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    word-break: break-all;
  }
  .meta {
    font-size: var(--fs-sm);
    margin-top: var(--sp-1);
  }
  .meta code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: 0.85em;
  }
  .chip {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 999px;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-xs);
  }
  .chip-ok {
    background: var(--accent-soft);
    color: var(--accent-strong);
  }
  .chip-warn {
    background: #fef3c7;
    color: #92400e;
  }
  .chip-off {
    background: var(--surface-2);
    color: var(--text-3);
  }
</style>
