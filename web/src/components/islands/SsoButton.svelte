<script lang="ts">
  // Shows the single sign-on button when the instance has a provider configured.
  import { onMount } from 'svelte';
  import { getSsoStatus } from '../../lib/api';

  let label = $state<string | null>(null);
  let only = $state(false);
  let href = $state('/auth/sso/start');

  onMount(async () => {
    const res = await getSsoStatus();
    if (res.ok && res.data?.enabled) {
      label = res.data.label ?? 'Entrar com sua conta';
      only = res.data.only;
    }
    const next = new URLSearchParams(window.location.search).get('next');
    if (next) href = `/auth/sso/start?next=${encodeURIComponent(next)}`;
  });
</script>

{#if label}
  <a class="btn btn-primary sso" {href}>{label}</a>
  {#if !only}<p class="or muted">ou</p>{/if}
{/if}

<style>
  .sso {
    display: flex;
    justify-content: center;
    width: 100%;
    margin-bottom: 0.5rem;
  }
  .or {
    text-align: center;
    margin: 0.5rem 0;
  }
</style>
