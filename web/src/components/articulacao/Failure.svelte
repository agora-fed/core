<script lang="ts">
  // What a view shows when a call fails: login, module off, or the server message.
  import Card from '../ui/Card.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import Button from '../ui/Button.svelte';
  import { loginHref, type Failure } from '../../lib/articulacao';

  let { failure, retry }: { failure: Failure; retry?: () => void } = $props();
</script>

<Card padding="none">
  {#if failure.kind === 'login'}
    <EmptyState
      icon="unlock"
      title="Entre na sua conta"
      description="A sala de articulação é para pessoas com conta no site."
    >
      {#snippet action()}
        <Button href={loginHref()} variant="primary">Entrar</Button>
      {/snippet}
    </EmptyState>
  {:else if failure.kind === 'module_off'}
    <EmptyState
      icon="info"
      title="Recurso indisponível"
      description="A sala de articulação não está ativa nesta organização."
    />
  {:else}
    <ErrorState message={failure.text} {retry} />
  {/if}
</Card>
