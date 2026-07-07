<script lang="ts">
  // Change-password form. Requires the current password so a stolen cookie
  // alone can't rotate the credential. On success the backend keeps the
  // current session alive but kills every other session + every OAuth token.
  import { changePassword } from '../../lib/api';
  import Input from '../ui/Input.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Icon from '../ui/Icon.svelte';

  let current = $state('');
  let next = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let result = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let nextLongEnough = $derived(next.length >= 8);
  let matches = $derived(next === confirm);
  let differs = $derived(next !== current);
  let canSubmit = $derived(
    !busy &&
      current.length > 0 &&
      nextLongEnough &&
      matches &&
      differs,
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!canSubmit) return;
    busy = true;
    result = null;
    const res = await changePassword(current, next);
    busy = false;
    if (res.success) {
      current = '';
      next = '';
      confirm = '';
      result = {
        kind: 'ok',
        text: 'Senha atualizada. As outras sessões e apps conectados foram desconectados.',
      };
    } else {
      result = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível trocar a senha.',
      };
    }
  }
</script>

<form onsubmit={submit} novalidate>
  <Input
    id="cp-current"
    label="Senha atual"
    type="password"
    autocomplete="current-password"
    bind:value={current}
    leading={lockIcon}
    required
  />
  <Input
    id="cp-new"
    label="Nova senha"
    type="password"
    autocomplete="new-password"
    bind:value={next}
    leading={keyIcon}
    required
    error={next.length > 0 && !nextLongEnough
      ? 'A nova senha deve ter ao menos 8 caracteres.'
      : undefined}
  />
  <Input
    id="cp-confirm"
    label="Confirmar nova senha"
    type="password"
    autocomplete="new-password"
    bind:value={confirm}
    leading={keyIcon}
    required
    error={confirm.length > 0 && !matches
      ? 'A confirmação não bate com a nova senha.'
      : undefined}
  />

  {#snippet lockIcon()}
    <Icon name="lock" size={16} />
  {/snippet}
  {#snippet keyIcon()}
    <Icon name="lock" size={16} />
  {/snippet}

  <Button
    type="submit"
    variant="primary"
    size="lg"
    fullWidth
    loading={busy}
    disabled={!canSubmit}
  >
    Trocar senha
  </Button>

  {#if result}
    <div class="alert">
      <Alert tone={result.kind === 'ok' ? 'success' : 'danger'}>
        {result.text}
      </Alert>
    </div>
  {/if}
</form>

<style>
  form {
    display: grid;
    gap: var(--sp-3);
  }
  .alert {
    margin-top: var(--sp-2);
  }
</style>
