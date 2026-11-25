<script lang="ts">
  // Paste a list of names; match them; mark as priorities or select on the board.
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';
  import Textarea from '../ui/Textarea.svelte';
  import { toast } from '../../lib/toasts';
  import { api, type ListMatch } from '../../lib/articulacao';

  interface Props {
    open: boolean;
    projectId: string;
    onclose: () => void;
    onpriorities: (ids: number[]) => void;
    onselect: (ids: number[]) => void;
  }
  let { open = $bindable(), projectId, onclose, onpriorities, onselect }: Props = $props();

  let text = $state('');
  let result = $state<ListMatch | null>(null);
  let busy = $state(false);

  async function check() {
    busy = true;
    const res = await api.matchList(projectId, text);
    busy = false;
    if (!res.success || !res.data) {
      toast.error(res.error?.message ?? 'Não foi possível conferir os nomes.');
      return;
    }
    result = res.data;
  }
  const ids = () => (result?.found ?? []).map((x) => x[0]);
</script>

<Modal bind:open title="Colar lista de nomes" {onclose}>
  <Textarea
    label="Nomes"
    bind:value={text}
    rows={8}
    hint="Um nome por linha, ou separados por vírgula. Serve para marcar prioritários ou selecionar deputados."
  />
  {#if result}
    <p><strong>{result.found.length}</strong> encontrados · <strong>{result.missed.length}</strong> sem correspondência</p>
    {#if result.missed.length}<p class="muted small">Sem correspondência: {result.missed.join('; ')}</p>{/if}
  {/if}
  {#snippet footer()}
    {#if result && result.found.length}
      <Button variant="secondary" onclick={() => onpriorities(ids())}>★ Marcar como prioritários</Button>
      <Button variant="secondary" onclick={() => onselect(ids())}>Selecionar no quadro</Button>
    {/if}
    <Button variant="primary" loading={busy} disabled={!text.trim()} onclick={check}>Conferir nomes</Button>
  {/snippet}
</Modal>

<style>
  .small {
    font-size: var(--fs-sm);
  }
</style>
