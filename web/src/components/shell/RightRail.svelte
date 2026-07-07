<script lang="ts">
  // Contextual right rail. Content varies by page via named slots. Falls back
  // to the platform pitch when no widgets are passed — better than empty.
  import type { Snippet } from 'svelte';
  import Icon from '../ui/Icon.svelte';

  interface Props {
    widgets?: Snippet;
  }
  let { widgets }: Props = $props();
</script>

<aside class="rail" aria-label="Contexto">
  {#if widgets}
    {@render widgets()}
  {:else}
    <section class="w">
      <h3><Icon name="chart" size={16} />Placar público</h3>
      <p>
        Cada pergunta feita a um mandato tem <strong>72 horas</strong> para
        resposta. Silêncio vira registro público — imutável.
      </p>
      <a class="link" href="/politicos"
        >Ver placar <Icon name="arrow-right" size={14} /></a
      >
    </section>
    <section class="w">
      <h3><Icon name="gavel" size={16} />Fediverso</h3>
      <p>
        DemocraciaBR fala ActivityPub. Sua conta funciona com qualquer
        instância Mastodon: <strong>@você@democracia.social.br</strong>.
      </p>
      <a class="link" href="/explorar"
        >Explorar <Icon name="arrow-right" size={14} /></a
      >
    </section>
    <section class="w small">
      <p>
        <a href="/sobre">Sobre</a> · <a href="/manifesto">Manifesto</a> ·
        <a href="/comunidade">Comunidade</a>
      </p>
    </section>
  {/if}
</aside>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-4) 0;
    position: sticky;
    top: calc(64px + var(--sp-2));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .w {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    padding: var(--sp-4);
  }
  .w h3 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-base);
    color: var(--text-1);
  }
  .w p {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--text-2);
    line-height: var(--lh-snug);
  }
  .w.small {
    background: transparent;
    border: 0;
    padding: 0;
    text-align: center;
  }
  .w.small p {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--accent);
    text-decoration: none;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
  }
  .link:hover {
    color: var(--accent-strong);
  }
</style>
