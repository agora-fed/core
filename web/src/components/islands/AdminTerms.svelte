<script lang="ts">
  import { onMount } from 'svelte';
  import { adminGetTerms, adminPatchTerms } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let body = $state('');
  let updatedAt = $state<string | null>(null);
  let saving = $state(false);

  onMount(async () => {
    const res = await adminGetTerms();
    loading = false;
    if (res.success && res.data) {
      body = res.data.body ?? '';
      updatedAt = res.data.updated_at;
    }
  });

  async function onSave() {
    if (!body.trim()) {
      toast.error('Texto obrigatório.');
      return;
    }
    saving = true;
    const res = await adminPatchTerms(body);
    saving = false;
    if (res.success) {
      toast.success('Termos atualizados.');
      updatedAt = new Date().toISOString();
    } else {
      toast.error(res.error?.message ?? 'Falha ao salvar.');
    }
  }
</script>

<Card>
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else}
    {#if updatedAt}
      <p class="muted small">Última atualização: {formatDate(updatedAt)}</p>
    {/if}
    <label class="fld">
      <span>Corpo (Markdown-lite; renderizado como texto)</span>
      <textarea bind:value={body} rows="20" placeholder="# Termos de Serviço&#10;&#10;..."></textarea>
    </label>
    <div class="a">
      <Button variant="primary" onclick={onSave} loading={saving} disabled={!body.trim()}>
        Salvar
      </Button>
    </div>
  {/if}
</Card>

<style>
  .small { font-size: var(--fs-sm); margin-bottom: var(--sp-3); }
  .fld {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
  }
  .fld textarea {
    padding: var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: var(--fs-sm);
    resize: vertical;
    min-height: 400px;
    line-height: 1.5;
  }
  .a { display: flex; justify-content: flex-end; margin-top: var(--sp-3); }
</style>
