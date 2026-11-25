<script lang="ts">
  // Create, rename or delete a custom column.
  import { untrack } from 'svelte';
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import { toast } from '../../lib/toasts';
  import { api, type Column } from '../../lib/articulacao';

  interface Props {
    open: boolean;
    projectId: string;
    column: Column | null;
    onclose: () => void;
    ondone: () => void;
  }
  let { open = $bindable(), projectId, column, onclose, ondone }: Props = $props();

  // The form starts from the value at open time; the modal is created anew each time.
  let name = $state(untrack(() => column?.name ?? ''));
  let busy = $state(false);

  async function run(call: Promise<{ success: boolean; error: { message: string } | null }>) {
    busy = true;
    const res = await call;
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível concluir.');
      return;
    }
    ondone();
  }
  const save = (e: SubmitEvent) => {
    e.preventDefault();
    run(column ? api.renameColumn(column.id, name) : api.addColumn(projectId, name));
  };
  const remove = () => {
    if (column && confirm(`Excluir a coluna "${column.name}"? Os deputados nela voltam para "Sem posição".`)) {
      run(api.deleteColumn(column.id));
    }
  };
</script>

<Modal bind:open title={column ? 'Editar coluna' : 'Nova coluna'} size="sm" {onclose}>
  <form id="column-form" onsubmit={save}>
    <Input label="Nome da coluna" bind:value={name} required maxlength={60} placeholder="Ex.: Indecisos PSD" />
  </form>
  {#snippet footer()}
    {#if column}<Button variant="danger" onclick={remove} disabled={busy}>Excluir coluna</Button>{/if}
    <span class="spacer"></span>
    <Button variant="ghost" onclick={onclose}>Cancelar</Button>
    <Button variant="primary" loading={busy} disabled={!name.trim()} onclick={() => (document.getElementById('column-form') as HTMLFormElement | null)?.requestSubmit()}>
      {column ? 'Salvar' : 'Criar coluna'}
    </Button>
  {/snippet}
</Modal>

<style>
  .spacer {
    flex: 1;
  }
</style>
