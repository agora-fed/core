<script lang="ts">
  // Formulário único de contato — o setor chega por ?setor= (links das
  // páginas institucionais) e vai no Subject do e-mail interno. Nenhum
  // endereço de e-mail aparece no HTML (anti-harvesting).
  import { sendContactMessage, type ContactSector } from '../../lib/api';

  const SECTORS: { value: ContactSector; label: string }[] = [
    { value: 'contato', label: 'Dúvidas gerais' },
    { value: 'lgpd', label: 'Privacidade / LGPD (Encarregado de dados)' },
    { value: 'moderacao', label: 'Moderação / denúncias' },
    { value: 'seguranca', label: 'Segurança (divulgação responsável)' },
    { value: 'imprensa', label: 'Imprensa' },
  ];

  function sectorFromUrl(): ContactSector {
    if (typeof window === 'undefined') return 'contato';
    const raw = new URLSearchParams(window.location.search).get('setor');
    return SECTORS.some((s) => s.value === raw) ? (raw as ContactSector) : 'contato';
  }

  let sector = $state<ContactSector>(sectorFromUrl());
  let name = $state('');
  let email = $state('');
  let subject = $state('');
  let message = $state('');
  let website = $state(''); // honeypot
  let busy = $state(false);
  let sent = $state(false);
  let error = $state('');

  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  let formValid = $derived(
    name.trim().length >= 2 &&
      emailValid &&
      subject.trim().length >= 3 &&
      message.trim().length >= 10,
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!formValid || busy) return;
    busy = true;
    error = '';
    const res = await sendContactMessage({
      sector,
      name: name.trim(),
      email: email.trim(),
      subject: subject.trim(),
      message: message.trim(),
      website,
    });
    busy = false;
    if (res.success) {
      sent = true;
    } else {
      error = res.error?.message ?? 'Não foi possível enviar. Tente novamente.';
    }
  }
</script>

{#if sent}
  <div class="card success" role="status">
    <h2>Mensagem enviada.</h2>
    <p class="muted">
      Recebemos sua mensagem e responderemos no e-mail informado. Obrigado
      por escrever.
    </p>
  </div>
{:else}
  <form class="contact-form" onsubmit={submit} novalidate>
    <div class="field">
      <label for="c-sector">Setor</label>
      <select id="c-sector" class="input" bind:value={sector}>
        {#each SECTORS as s (s.value)}
          <option value={s.value}>{s.label}</option>
        {/each}
      </select>
    </div>

    <div class="field">
      <label for="c-name">Seu nome</label>
      <input id="c-name" class="input" type="text" autocomplete="name" bind:value={name} required />
    </div>

    <div class="field">
      <label for="c-email">Seu e-mail (para resposta)</label>
      <input
        id="c-email"
        class="input"
        type="email"
        autocomplete="email"
        bind:value={email}
        aria-invalid={email.length > 0 && !emailValid}
        required
      />
      {#if email.length > 0 && !emailValid}
        <p class="hint hint-error">Informe um e-mail válido.</p>
      {/if}
    </div>

    <div class="field">
      <label for="c-subject">Assunto</label>
      <input id="c-subject" class="input" type="text" maxlength="180" bind:value={subject} required />
    </div>

    <div class="field">
      <label for="c-message">Mensagem</label>
      <textarea id="c-message" class="input" rows="7" maxlength="5000" bind:value={message} required
      ></textarea>
      <p class="hint muted">Mínimo de 10 caracteres. Não inclua senhas ou dados sensíveis.</p>
    </div>

    <!-- Honeypot: fora do fluxo visual e do tab; bots preenchem, humanos não. -->
    <div class="hp" aria-hidden="true">
      <label for="c-website">Website</label>
      <input id="c-website" type="text" tabindex="-1" autocomplete="off" bind:value={website} />
    </div>

    {#if error}
      <p class="hint hint-error" role="alert">{error}</p>
    {/if}

    <button class="btn btn-primary btn-lg block" type="submit" disabled={!formValid || busy}>
      {busy ? 'Enviando…' : 'Enviar mensagem'}
    </button>
  </form>
{/if}

<style>
  .contact-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .success {
    padding: 1.5rem;
    text-align: center;
  }
  .success h2 {
    margin-top: 0;
  }
  .hp {
    position: absolute;
    left: -9999px;
    width: 1px;
    height: 1px;
    overflow: hidden;
  }
  textarea.input {
    resize: vertical;
  }
</style>
