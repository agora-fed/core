<script lang="ts">
  // Registration: e-mail + senha + CPF, with client-side CPF check-digit validation.
  import { apiPost } from '../../lib/api';
  import { formatCpf, isValidCpf, onlyDigits } from '../../lib/cpf';

  interface SessionData {
    citizen_id?: string;
  }

  let email = $state('');
  let password = $state('');
  let cpf = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let cpfTouched = $state(false);

  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  let passwordValid = $derived(password.length >= 8);
  let cpfValid = $derived(isValidCpf(cpf));
  let valid = $derived(emailValid && passwordValid && cpfValid);

  function onCpfInput(event: Event) {
    const el = event.target as HTMLInputElement;
    cpf = formatCpf(el.value);
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    serverError = null;
    busy = true;

    const res = await apiPost<SessionData>('/api/v1/auth/register', {
      email: email.trim(),
      password,
      cpf: onlyDigits(cpf),
    });
    busy = false;

    if (res.success) {
      if (res.data?.citizen_id) {
        try {
          localStorage.setItem('dsoc_citizen', res.data.citizen_id);
        } catch {
          /* storage may be blocked; session cookie still set */
        }
      }
      window.location.href = '/';
    } else {
      serverError =
        res.error?.message ??
        'Não foi possível criar sua conta. Tente novamente.';
    }
  }
</script>

<form class="auth-form" onsubmit={submit} novalidate>
  <div class="field">
    <label for="r-email">E-mail</label>
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

  <div class="field">
    <label for="r-password">Senha</label>
    <input
      id="r-password"
      class="input"
      type="password"
      autocomplete="new-password"
      bind:value={password}
      aria-invalid={password.length > 0 && !passwordValid}
      aria-describedby="r-pass-hint"
      required
    />
    <p
      id="r-pass-hint"
      class={`hint ${password.length > 0 && !passwordValid ? 'hint-error' : 'muted'}`}
    >
      Mínimo de 8 caracteres.
    </p>
  </div>

  <div class="field">
    <label for="r-cpf">CPF</label>
    <input
      id="r-cpf"
      class="input"
      type="text"
      inputmode="numeric"
      autocomplete="off"
      value={cpf}
      oninput={onCpfInput}
      onblur={() => (cpfTouched = true)}
      aria-invalid={cpfTouched && cpf.length > 0 && !cpfValid}
      aria-describedby="r-cpf-hint"
      placeholder="000.000.000-00"
      maxlength="14"
      required
    />
    {#if cpf.length > 0 && cpfValid}
      <p id="r-cpf-hint" class="hint hint-ok">✓ CPF válido.</p>
    {:else if (cpfTouched || onlyDigits(cpf).length === 11) && cpf.length > 0}
      <p id="r-cpf-hint" class="hint hint-error">
        CPF inválido. Verifique os dígitos.
      </p>
    {:else}
      <p id="r-cpf-hint" class="hint muted">
        Usado apenas para verificar a sua identidade cívica.
      </p>
    {/if}
  </div>

  <button
    class="btn btn-primary btn-lg block"
    type="submit"
    disabled={!valid || busy}
  >
    {busy ? 'Criando conta…' : 'Criar minha conta'}
  </button>

  {#if serverError}
    <p class="note error" role="alert">{serverError}</p>
  {/if}

  <p class="alt muted">
    Já tem conta? <a href="/entrar">Entrar</a>
  </p>
</form>

<style>
  .auth-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .note {
    margin: 0.85rem 0 0;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .alt {
    margin-top: 1rem;
    text-align: center;
    font-size: 0.95rem;
  }
</style>
