<script lang="ts">
  // /abaixo-assinado/confirmar/: the page the confirmation e-mail links to.
  // It sends the proof to the server once, then shows the result.
  import { onMount } from 'svelte';
  import { confirmSignature, proofFromQuery, type Confirmed } from '../../lib/petitions';

  let phase = $state<'working' | 'done' | 'failed'>('working');
  let result = $state<Confirmed | null>(null);
  let message = $state('');

  onMount(async () => {
    const proof = proofFromQuery(window.location.search);
    // Drop the secret from the address bar and the history.
    window.history.replaceState(null, '', window.location.pathname);
    if (!proof) {
      phase = 'failed';
      message = 'Este link está incompleto. Abra de novo o link do e-mail.';
      return;
    }
    const res = await confirmSignature(proof);
    if (res.success && res.data) {
      result = res.data;
      phase = 'done';
    } else {
      phase = 'failed';
      message = res.error?.message ?? 'Não foi possível confirmar.';
    }
  });
</script>

{#if phase === 'working'}
  <h1>Confirmando sua assinatura…</h1>
{:else if phase === 'done' && result}
  {@const r = result}
  <h1>Assinatura confirmada!</h1>
  <p>Obrigado. Agora são <strong>{r.signature_count.toLocaleString('pt-BR')}</strong> assinaturas
    de uma meta de {r.threshold.toLocaleString('pt-BR')}.{#if r.reached} A meta foi alcançada!{/if}</p>
  {#if r.account_created}
    <p>Também criamos sua conta. Você vai receber outro e-mail com um link para escolher sua senha.</p>
  {/if}
  <p class="row">
    <a class="btn btn-primary" href={`/abaixo-assinado/?id=${encodeURIComponent(r.petition_id)}`}>Ver o abaixo-assinado</a>
    <a class="btn" href="/abaixo-assinados">Ver outros</a>
  </p>
{:else}
  <h1>Não foi possível confirmar</h1>
  <p>{message}</p>
  <p><a href="/abaixo-assinados">Ver abaixo-assinados</a></p>
{/if}

<style>
  .row { display: flex; gap: 0.5rem; flex-wrap: wrap; }
</style>
