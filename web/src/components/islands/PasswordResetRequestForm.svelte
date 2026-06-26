<script lang="ts">
  // "Esqueci minha senha" — step 1: ask for an e-mail, ALWAYS report success regardless of
  // whether the address is registered (the backend is enumeration-resistant; the front mirrors
  // the same posture so a wrong address does not reveal anything).
  import { requestPasswordReset } from '../../lib/api';

  let email = $state('');
  let busy = $state(false);
  let sent = $state(false);
  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!emailValid || busy) return;
    busy = true;
    await requestPasswordReset(email);
    busy = false;
    sent = true;
  }
</script>

{#if sent}
  <div class="card success" role="status">
    <h2>Verifique sua caixa de entrada.</h2>
    <p class="muted">
      Se houver uma conta com este e-mail, enviamos um link para redefinir
      sua senha. O link expira em 1 hora.
    </p>
    <p class="muted">
      Não chegou nada? Olhe a pasta de spam ou
      <a
        href="#"
        onclick={(e) => {
          e.preventDefault();
          sent = false;
        }}
      >tente outro e-mail</a>.
    </p>
  </div>
{:else}
  <form class="auth-form" onsubmit={submit} novalidate>
    <div class="field">
      <label for="r-email">E-mail da sua conta</label>
      <input
        id="r-email"
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
    <button
      class="btn btn-primary btn-lg block"
      type="submit"
      disabled={!emailValid || busy}
    >
      {busy ? 'Enviando…' : 'Enviar link de redefinição'}
    </button>
    <p class="alt muted">
      <a href="/entrar">Voltar para o login</a>
    </p>
  </form>
{/if}

<style>
  .auth-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .alt {
    margin-top: 1rem;
    text-align: center;
    font-size: 0.95rem;
  }
  .success {
    padding: 1.5rem;
    text-align: center;
  }
  .success h2 {
    margin-top: 0;
  }
</style>
