<script lang="ts">
  // Router island for /politicos/. If the URL carries ?id=<uuid> we mount the
  // MandateDetail profile (works for federal, estadual, AND municipal — the
  // island fetches from the API so no SSG is required). Without the query, we
  // mount the filtered browser.
  //
  // This exists because /politicos/[mandate].astro's SSG only pre-generates
  // federal+estadual (~1.7k pages, ~15s build). Emitting HTML for the ~68k
  // municipal mandates would push the build past 9 minutes and the container
  // image past 500 MB — so municipal profile pages route through this fallback.
  import { onMount } from 'svelte';
  import PoliticosBrowser from './PoliticosBrowser.svelte';
  import MandateDetail from './MandateDetail.svelte';

  let mode = $state<'browser' | 'profile'>('browser');
  let mandateId = $state<string | null>(null);

  onMount(() => {
    if (typeof window === 'undefined') return;
    const p = new URLSearchParams(window.location.search);
    const id = p.get('id');
    if (id && /^[0-9a-f-]{36}$/i.test(id)) {
      mandateId = id;
      mode = 'profile';
    }
  });
</script>

{#if mode === 'profile' && mandateId}
  <MandateDetail mandateId={mandateId} />
{:else}
  <PoliticosBrowser />
{/if}
