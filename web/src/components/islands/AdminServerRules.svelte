<script lang="ts">
  // CRUD simples de server_rule. Ordenação por `ordinal` — o admin edita
  // esse número. Sem drag-and-drop nessa fatia (menor código, mesmo efeito).
  import { onMount } from 'svelte';
  import {
    adminListRules,
    adminCreateRule,
    adminUpdateRule,
    adminDeleteRule,
    type ServerRuleDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<ServerRuleDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let newText = $state('');
  let newOrdinal = $state(0);
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListRules();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar.';
    }
  }
  onMount(reload);

  async function onCreate(e: SubmitEvent) {
    e.preventDefault();
    const t = newText.trim();
    if (!t) return;
    creating = true;
    const res = await adminCreateRule(t, newOrdinal);
    creating = false;
    if (res.success && res.data) {
      items = [...items, res.data].sort((a, b) => a.ordinal - b.ordinal || a.created_at.localeCompare(b.created_at));
      newText = '';
      newOrdinal = 0;
      toast.success('Regra criada.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao criar.');
    }
  }

  async function onSave(r: ServerRuleDto, text: string, ordinal: number) {
    if (busy.has(r.id)) return;
    busy = new Set(busy).add(r.id);
    const res = await adminUpdateRule(r.id, { text, ordinal });
    const done = new Set(busy);
    done.delete(r.id);
    busy = done;
    if (res.success) {
      toast.success('Regra atualizada.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha ao salvar.');
    }
  }

  async function onDelete(r: ServerRuleDto) {
    if (busy.has(r.id)) return;
    if (!confirm('Apagar esta regra?')) return;
    busy = new Set(busy).add(r.id);
    const res = await adminDeleteRule(r.id);
    const done = new Set(busy);
    done.delete(r.id);
    busy = done;
    if (res.success) {
      items = items.filter((i) => i.id !== r.id);
      toast.success('Removida.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }
</script>

<Card>
  <h2 class="sub">Nova regra</h2>
  <form onsubmit={onCreate} class="form">
    <label class="fld wide">
      <span>Texto</span>
      <textarea bind:value={newText} maxlength="4000" rows="2" placeholder="Ex.: Sem apologia a genocídio."></textarea>
    </label>
    <label class="fld">
      <span>Ordem</span>
      <input type="number" bind:value={newOrdinal} step="1" />
    </label>
    <Button type="submit" variant="primary" loading={creating} disabled={!newText.trim()}>Criar</Button>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card><EmptyState icon="shield" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="shield"
        title="Sem regras"
        description="Crie a primeira acima. Assim que existir, aparece pra visitantes."
      />
    </Card>
  {:else}
    <ul class="rows">
      {#each items as r (r.id)}
        <li>
          <Card>
            <form
              class="row"
              onsubmit={(e) => {
                e.preventDefault();
                const form = e.currentTarget as HTMLFormElement;
                const t = (form.querySelector('textarea') as HTMLTextAreaElement).value;
                const o = Number((form.querySelector('input[type=number]') as HTMLInputElement).value);
                void onSave(r, t, o);
              }}
            >
              <input type="number" class="ord" value={r.ordinal} step="1" />
              <textarea class="txt" rows="2" maxlength="4000">{r.text}</textarea>
              <div class="a">
                <Button type="submit" variant="ghost" size="sm" loading={busy.has(r.id)}>Salvar</Button>
                <Button type="button" variant="danger" size="sm" onclick={() => onDelete(r)} loading={busy.has(r.id)}>Apagar</Button>
              </div>
            </form>
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
    grid-template-columns: 1fr 100px auto;
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
  .fld.wide { grid-column: 1 / 2; }
  .fld input,
  .fld textarea {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .fld textarea { resize: vertical; }
  @media (max-width: 720px) {
    .form { grid-template-columns: 1fr; }
  }
  .list { margin-top: var(--sp-4); }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    display: grid;
    grid-template-columns: 80px 1fr auto;
    gap: var(--sp-3);
    align-items: start;
  }
  .row .ord,
  .row .txt {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .row .txt { resize: vertical; min-height: 60px; }
  .a { display: flex; flex-direction: column; gap: var(--sp-1); }
  @media (max-width: 700px) {
    .row { grid-template-columns: 1fr; }
    .a { flex-direction: row; }
  }
</style>
