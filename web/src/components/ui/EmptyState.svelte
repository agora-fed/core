<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    icon?: string;
    title: string;
    description?: string;
    action?: Snippet;
  }
  let { icon = 'info', title, description, action }: Props = $props();
</script>

<div class="es" role="status">
  <span class="ic"><Icon name={icon} size={32} /></span>
  <h3>{title}</h3>
  {#if description}<p>{description}</p>{/if}
  {#if action}
    <div class="act">{@render action()}</div>
  {/if}
</div>

<style>
  .es {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: var(--sp-12) var(--sp-4);
    gap: var(--sp-3);
    color: var(--text-2);
  }
  .ic {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 64px;
    height: 64px;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--text-3);
  }
  h3 {
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  p {
    margin: 0;
    max-width: 32rem;
    color: var(--text-3);
    font-size: var(--fs-sm);
    line-height: var(--lh-snug);
  }
  .act {
    margin-top: var(--sp-3);
  }
</style>
