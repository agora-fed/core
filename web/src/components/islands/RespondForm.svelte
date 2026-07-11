<script lang="ts">
  // Reply-to-respond (0.30): o gabinete chega aqui pelo link assinado do
  // e-mail de aviso (?sla=…&t=…) e responde SEM criar conta — a posse do
  // token (enviado só à caixa oficial do mandato) é a autorização. A
  // resposta entra no SLA e vira desfecho público permanente no placar.
  import { onMount } from 'svelte';
  import {
    getRespondContext,
    submitRespond,
    type RespondContextDto,
  } from '../../lib/api';

  let phase = $state<'loading' | 'form' | 'done' | 'invalid' | 'resolved'>('loading');
  let ctx = $state<RespondContextDto | null>(null);
  let sla = $state('');
  let token = $state('');
  let body = $state('');
  let committed = $state(false);
  let busy = $state(false);
  let error = $state('');

  onMount(async () => {
    const params = new URLSearchParams(window.location.search);
    sla = params.get('sla') ?? '';
    token = params.get('t') ?? '';
    if (!sla || !token) {
      phase = 'invalid';
      return;
    }
    const res = await getRespondContext(sla, token);
    if (!res.success || !res.data) {
      phase = 'invalid';
      return;
    }
    ctx = res.data;
    phase = ctx.status === 'pending' ? 'form' : 'resolved';
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || body.trim().length < 10) return;
    busy = true;
    error = '';
    const res = await submitRespond({
      sla_id: sla,
      token,
      body: body.trim(),
      committed,
    });
    busy = false;
    if (res.success) {
      phase = 'done';
    } else if (res.error?.code === 'already_resolved') {
      phase = 'resolved';
    } else {
      error = res.error?.message ?? 'Não foi possível registrar. Tente novamente.';
    }
  }
</script>

{#if phase === 'loading'}
  <p class="muted">Verificando o link…</p>
{:else if phase === 'invalid'}
  <div class="card">
    <h2>Link inválido</h2>
    <p class="muted">
      Este link de resposta não é válido. Use exatamente o link recebido no
      e-mail oficial do gabinete — ou responda pela página da demanda, com
      a conta de operador do mandato.
    </p>
  </div>
{:else if phase === 'resolved'}
  <div class="card">
    <h2>Este prazo já foi resolvido</h2>
    <p class="muted">
      O desfecho deste prazo é permanente e já consta no placar público —
      não pode ser alterado.
    </p>
  </div>
{:else if phase === 'done'}
  <div class="card success" role="status">
    <h2>Resposta registrada ✓</h2>
    <p class="muted">
      A resposta do gabinete foi registrada e entra no placar público como
      <strong>prazo respondido</strong>. Obrigado por responder à cidadania.
    </p>
  </div>
{:else if ctx}
  <form class="respond-form" onsubmit={submit}>
    <div class="ctx">
      <p>
        <strong>{ctx.mandate_display_name ?? 'Gabinete'}</strong>, a demanda
        cidadã <strong>“{ctx.proposal_title}”</strong> aguarda resposta até
        <time datetime={ctx.due_at}>
          {new Date(ctx.due_at).toLocaleDateString('pt-BR')}</time>.
      </p>
      <p class="muted">
        A resposta é pública, permanente e entra no placar do mandato. Sem
        resposta até o prazo, o silêncio fica registrado — com os recibos
        dos avisos.
      </p>
    </div>
    <div class="field">
      <label for="r-body">Resposta oficial do gabinete</label>
      <textarea id="r-body" class="input" rows="8" maxlength="10000" bind:value={body} required
      ></textarea>
      <p class="hint muted">Mínimo de 10 caracteres.</p>
    </div>
    <label class="commit">
      <input type="checkbox" bind:checked={committed} />
      Esta resposta assume um <strong>compromisso concreto</strong> (entra
      no placar como “ação assumida”, cobrável no ciclo seguinte).
    </label>
    {#if error}
      <p class="hint hint-error" role="alert">{error}</p>
    {/if}
    <button class="btn btn-primary btn-lg block" type="submit" disabled={busy || body.trim().length < 10}>
      {busy ? 'Registrando…' : 'Registrar resposta pública'}
    </button>
  </form>
{/if}

<style>
  .respond-form,
  .card {
    display: block;
  }
  .card {
    padding: 1.5rem;
    text-align: center;
  }
  .card h2 {
    margin-top: 0;
  }
  .ctx {
    margin-bottom: 1.25rem;
  }
  .commit {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    margin: 0.75rem 0 1rem;
    font-size: 0.95rem;
  }
  .block {
    width: 100%;
  }
  textarea.input {
    resize: vertical;
  }
</style>
