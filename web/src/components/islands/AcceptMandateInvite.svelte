<script lang="ts">
  // Aceitar convite de mandato. Fluxo:
  //   1) Ao montar: lê ?token=... da URL e chama GET /api/v1/mandate-invites/{token} para
  //      buscar o resumo do mandato (nome, partido/UF, cargo).
  //   2) Renderiza o convite + formulário (senha, CPF, nome público).
  //   3) POST /accept ⇒ cria conta + binding + sessão; redireciona para /painel-mandato.
  //
  // Espelha o padrão de RegisterForm.svelte (mesmo estilo de campos, mesmo tratamento de
  // erro) e de PasswordResetConfirmForm.svelte (mesma leitura do token da querystring).
  import { onMount } from 'svelte';
  import {
    getMandateInvite,
    acceptMandateInvite,
    type MandateInviteSummaryDto,
  } from '../../lib/api';
  import { formatCpf, isValidCpf, onlyDigits } from '../../lib/cpf';

  let token = $state('');
  let loading = $state(true);
  let invite = $state<MandateInviteSummaryDto | null>(null);
  let loadError = $state<string | null>(null);

  let password = $state('');
  let displayName = $state('');
  let cpf = $state('');
  let cpfTouched = $state(false);
  let busy = $state(false);
  let serverError = $state<string | null>(null);

  let passwordValid = $derived(password.length >= 8);
  let cpfValid = $derived(isValidCpf(cpf));
  let nameValid = $derived(displayName.trim().length >= 1 && displayName.trim().length <= 80);
  let valid = $derived(passwordValid && cpfValid && nameValid && invite !== null);

  onMount(async () => {
    try {
      const parts = window.location.pathname.split('/').filter(Boolean);
      // Aceita tanto /convite?token=... quanto /convite/{token} (caso um adapter SSR seja
      // instalado no futuro). Prioridade: querystring — é o formato emitido pelo backend hoje.
      const q = new URLSearchParams(window.location.search).get('token');
      const pathTok = parts[0] === 'convite' && parts.length > 1 ? parts[1] : null;
      token = q ?? pathTok ?? '';
    } catch {
      token = '';
    }
    if (!token) {
      loadError = 'Link inválido. Peça um novo convite ao administrador.';
      loading = false;
      return;
    }
    const res = await getMandateInvite(token);
    loading = false;
    if (res.ok && res.data) {
      invite = res.data;
    } else {
      loadError =
        res.error ??
        'Este convite pode ter expirado, sido revogado ou já ter sido usado.';
    }
  });

  function onCpfInput(event: Event) {
    const el = event.target as HTMLInputElement;
    cpf = formatCpf(el.value);
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    serverError = null;

    const res = await acceptMandateInvite(token, {
      password,
      cpf: onlyDigits(cpf),
      display_name: displayName.trim(),
    });
    busy = false;

    if (res.success) {
      if (res.data?.citizen_id) {
        try {
          localStorage.setItem('dsoc_citizen', res.data.citizen_id);
          if (res.data.public_handle) {
            localStorage.setItem('dsoc_handle', res.data.public_handle);
          }
        } catch {
          /* storage may be blocked; session cookie still set */
        }
      }
      window.location.href = '/painel-mandato';
    } else {
      serverError =
        res.error?.message ??
        'Não foi possível concluir o cadastro. Tente novamente.';
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando convite…</p>
{:else if loadError}
  <div class="card error" role="alert">
    <h2>Convite indisponível.</h2>
    <p class="muted">{loadError}</p>
    <p class="muted">
      Se você espera um convite, peça ao(à) administrador(a) para reenviá-lo.
    </p>
  </div>
{:else if invite}
  <div class="invite-summary">
    <p class="muted">Você foi convidado(a) a assumir o mandato de:</p>
    <p class="mandate">
      <strong>{invite.mandate_display_name}</strong>
      <span class="muted">
        {invite.party ?? '—'}/{invite.uf ?? '—'} · {invite.office}
      </span>
    </p>
    <p class="hint muted">
      Ao concluir, sua identidade fica vinculada ao mandato acima (nível
      <em>directory</em>), sua conta é criada com perfil público, e você
      entra direto no painel do(a) parlamentar para responder às demandas
      dirigidas a você.
    </p>
  </div>

  <form class="auth-form" onsubmit={submit} novalidate>
    <div class="field">
      <label for="a-name">Nome público (como aparece no perfil)</label>
      <input
        id="a-name"
        class="input"
        type="text"
        autocomplete="name"
        bind:value={displayName}
        aria-invalid={displayName.length > 0 && !nameValid}
        maxlength="80"
        required
      />
      <p class={`hint ${displayName.length > 0 && !nameValid ? 'hint-error' : 'muted'}`}>
        Entre 1 e 80 caracteres.
      </p>
    </div>

    <div class="field">
      <label for="a-password">Senha</label>
      <input
        id="a-password"
        class="input"
        type="password"
        autocomplete="new-password"
        bind:value={password}
        aria-invalid={password.length > 0 && !passwordValid}
        required
      />
      <p class={`hint ${password.length > 0 && !passwordValid ? 'hint-error' : 'muted'}`}>
        Mínimo de 8 caracteres.
      </p>
    </div>

    <div class="field">
      <label for="a-cpf">CPF</label>
      <input
        id="a-cpf"
        class="input"
        type="text"
        inputmode="numeric"
        autocomplete="off"
        value={cpf}
        oninput={onCpfInput}
        onblur={() => (cpfTouched = true)}
        aria-invalid={cpfTouched && cpf.length > 0 && !cpfValid}
        placeholder="000.000.000-00"
        maxlength="14"
        required
      />
      {#if cpf.length > 0 && cpfValid}
        <p class="hint hint-ok">✓ CPF válido.</p>
      {:else if (cpfTouched || onlyDigits(cpf).length === 11) && cpf.length > 0}
        <p class="hint hint-error">CPF inválido. Verifique os dígitos.</p>
      {:else}
        <p class="hint muted">Usado apenas para verificar sua identidade cívica.</p>
      {/if}
    </div>

    <button
      class="btn btn-primary btn-lg block"
      type="submit"
      disabled={!valid || busy}
    >
      {busy ? 'Criando conta…' : 'Aceitar convite e assumir o mandato'}
    </button>

    {#if serverError}
      <p class="note error" role="alert">{serverError}</p>
    {/if}
  </form>
{/if}

<style>
  .auth-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .invite-summary {
    padding: 0.9rem 1rem;
    background: var(--c-green-soft);
    border: 1px solid var(--c-green-dark);
    border-radius: 8px;
    margin-bottom: 1.2rem;
  }
  .invite-summary .mandate {
    margin: 0.35rem 0;
    display: grid;
    gap: 0.15rem;
  }
  .invite-summary .mandate strong {
    font-size: 1.05rem;
  }
  .invite-summary .hint {
    margin: 0.6rem 0 0;
    font-size: 0.88rem;
  }
  .note {
    margin-top: 0.85rem;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .error {
    padding: 1.5rem;
    text-align: center;
  }
  .error h2 {
    margin-top: 0;
  }
</style>
