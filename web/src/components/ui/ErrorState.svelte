<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    title?: string;
    message?: string;
    retry?: () => void;
    action?: Snippet;
  }
  let {
    title = 'Algo deu errado',
    message = 'Não foi possível carregar. Tente de novo em instantes.',
    retry,
    action,
  }: Props = $props();
</script>

<div class="es" role="alert">
  <span class="ic"><Icon name="alert" size={28} /></span>
  <h3>{title}</h3>
  <p>{message}</p>
  {#if retry}
    <button type="button" onclick={retry}>Tentar novamente</button>
  {/if}
  {#if action}
    <div class="act">{@render action()}</div>
  {/if}
</div>

<style>
  .es {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: var(--sp-10) var(--sp-4);
    gap: var(--sp-2);
    color: var(--text-2);
    background: var(--danger-soft);
    border: 1px solid color-mix(in srgb, var(--danger) 20%, transparent);
    border-radius: var(--r-base);
  }
  .ic {
    color: var(--danger);
    margin-bottom: var(--sp-1);
  }
  h3 {
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  p {
    margin: 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  button {
    margin-top: var(--sp-3);
    padding: var(--sp-2) var(--sp-4);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-full);
    font: inherit;
    font-weight: var(--fw-semibold);
    color: var(--text-1);
    cursor: pointer;
  }
  button:hover {
    background: var(--surface-2);
  }
</style>
