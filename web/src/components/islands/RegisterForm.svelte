<script lang="ts">
  // Registration: e-mail + senha + CPF, with client-side CPF check-digit validation.
  // F1.3: toggle Cidadão | Político/Candidata. In politician mode the user picks their
  // mandate from a searchable list; on submit we hit /auth/register/politician which
  // enforces `email == mandate.public_email` server-side. F1.4 kicks in automatically:
  // the server sets is_public=true + writes the identity binding.
  import { onMount } from 'svelte';
  import {
    register,
    registerPolitician,
    getMandates,
    DEFAULT_ORG_ID,
    type MandateDto,
  } from '../../lib/api';
  import { formatCpf, isValidCpf, onlyDigits } from '../../lib/cpf';

  type Role = 'cidadao' | 'politico';

  let role = $state<Role>('cidadao');
  let email = $state('');
  let password = $state('');
  let cpf = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let cpfTouched = $state(false);

  // Politician-mode state.
  let mandateSearch = $state('');
  let selectedMandate = $state<MandateDto | null>(null);
  let mandates = $state<MandateDto[]>([]);
  let mandatesLoaded = $state(false);
  let mandateListOpen = $state(false);

  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  let passwordValid = $derived(password.length >= 8);
  let cpfValid = $derived(isValidCpf(cpf));
  let valid = $derived(
    emailValid
      && passwordValid
      && cpfValid
      && (role === 'cidadao' || selectedMandate !== null),
  );

  // Filtered candidates for the mandate picker.
  let mandateResults = $derived.by(() => {
    const q = mandateSearch.trim().toLowerCase();
    if (q.length < 2) return [] as MandateDto[];
    return mandates
      .filter((m) => {
        const hay = `${m.display_name} ${m.party ?? ''} ${m.uf ?? ''} ${m.office}`.toLowerCase();
        return hay.includes(q);
      })
      .slice(0, 12);
  });

  async function ensureMandates() {
    if (mandatesLoaded) return;
    const r = await getMandates(DEFAULT_ORG_ID, 500);
    if (r.ok && r.data) {
      mandates = r.data;
    }
    mandatesLoaded = true;
  }

  function onCpfInput(event: Event) {
    const el = event.target as HTMLInputElement;
    cpf = formatCpf(el.value);
  }

  function pickMandate(m: MandateDto) {
    selectedMandate = m;
    mandateSearch = m.display_name;
    mandateListOpen = false;
    // Auto-fill the e-mail with the public_email so the check on the server passes.
    // The user can still edit it if they know a different address is on file, but 99% of
    // the time this saves a lookup.
    if (m.public_email && !email) email = m.public_email;
  }

  async function switchRole(next: Role) {
    role = next;
    serverError = null;
    if (next === 'politico') await ensureMandates();
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    serverError = null;
    busy = true;

    const res = role === 'politico' && selectedMandate
      ? await registerPolitician(email, password, onlyDigits(cpf), selectedMandate.id)
      : await register(email, password, onlyDigits(cpf));
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
      // Politicians land straight in their painel; citizens go through the welcome page.
      window.location.href = role === 'politico' ? '/painel-mandato' : '/bem-vinda';
    } else {
      serverError =
        res.error?.message ??
        'Não foi possível criar sua conta. Tente novamente.';
    }
  }
</script>

<form class="auth-form" onsubmit={submit} novalidate>
  <fieldset class="role-picker" aria-label="Tipo de conta">
    <legend class="muted role-legend">Você está entrando como</legend>
    <div class="role-tabs">
      <button
        type="button"
        class={`role-tab ${role === 'cidadao' ? 'active' : ''}`}
        onclick={() => switchRole('cidadao')}
      >
        <strong>Cidadã(o)</strong>
        <span class="muted role-hint">Propor, votar, cobrar</span>
      </button>
      <button
        type="button"
        class={`role-tab ${role === 'politico' ? 'active' : ''}`}
        onclick={() => switchRole('politico')}
      >
        <strong>Político(a) / Candidata(o)</strong>
        <span class="muted role-hint">Responder, prestar contas</span>
      </button>
    </div>
  </fieldset>

  {#if role === 'politico'}
    <div class="field">
      <label for="r-mandate">Seu mandato</label>
      {#if selectedMandate}
        <div class="selected-mandate">
          {#if selectedMandate.avatar_url}
            <img class="mandate-avatar" src={selectedMandate.avatar_url} alt="" />
          {/if}
          <div class="selected-meta">
            <strong>{selectedMandate.display_name}</strong>
            <span class="muted">
              {selectedMandate.party ?? '—'}/{selectedMandate.uf ?? '—'} ·
              {selectedMandate.office}
            </span>
          </div>
          <button
            type="button"
            class="btn-link"
            onclick={() => {
              selectedMandate = null;
              mandateSearch = '';
              mandateListOpen = true;
            }}
          >trocar</button>
        </div>
      {:else}
        <input
          id="r-mandate"
          class="input"
          type="search"
          bind:value={mandateSearch}
          onfocus={() => (mandateListOpen = true)}
          placeholder="Buscar por nome, partido, UF ou cargo…"
          autocomplete="off"
          required
        />
        {#if mandateListOpen && mandateResults.length > 0}
          <ul class="mandate-list">
            {#each mandateResults as m (m.id)}
              <li>
                <button
                  type="button"
                  class="mandate-option"
                  onclick={() => pickMandate(m)}
                >
                  {#if m.avatar_url}
                    <img class="mandate-avatar" src={m.avatar_url} alt="" loading="lazy" />
                  {:else}
                    <span class="mandate-avatar mandate-avatar-ph">👤</span>
                  {/if}
                  <span class="mandate-option-meta">
                    <strong>{m.display_name}</strong>
                    <span class="muted">
                      {m.party ?? '—'}/{m.uf ?? '—'} · {m.office}
                    </span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if mandateListOpen && mandateSearch.trim().length >= 2}
          <p class="hint muted">Nenhum mandato bate essa busca.</p>
        {/if}
      {/if}
      <p class="hint muted">
        Use o e-mail oficial do gabinete cadastrado na Câmara/Senado/TSE — é assim
        que confirmamos que o mandato é seu.
      </p>
    </div>
  {/if}

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
    {busy
      ? 'Criando conta…'
      : role === 'politico'
        ? 'Criar conta e assumir o mandato'
        : 'Criar minha conta'}
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
  .role-picker {
    border: none;
    padding: 0;
    margin: 0 0 1.2rem;
  }
  .role-legend {
    font-size: 0.85rem;
    margin-bottom: 0.4rem;
    padding: 0;
  }
  .role-tabs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
  }
  .role-tab {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 0.7rem 0.85rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 8px;
    cursor: pointer;
    font-family: inherit;
    text-align: left;
    gap: 0.15rem;
    color: var(--c-text);
  }
  .role-tab:hover {
    background: var(--c-bg);
  }
  .role-tab.active {
    background: var(--c-green-soft);
    border-color: var(--c-green-dark);
  }
  .role-tab strong {
    font-size: 0.95rem;
  }
  .role-hint {
    font-size: 0.8rem;
  }
  .mandate-list {
    list-style: none;
    padding: 0;
    margin: 0.3rem 0 0;
    border: 1px solid var(--c-border);
    border-radius: 8px;
    max-height: 260px;
    overflow-y: auto;
    background: var(--c-paper);
  }
  .mandate-option {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    width: 100%;
    padding: 0.55rem 0.7rem;
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--c-border);
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    color: var(--c-text);
  }
  .mandate-option:last-child {
    border-bottom: none;
  }
  .mandate-option:hover,
  .mandate-option:focus {
    background: var(--c-bg);
  }
  .mandate-option-meta {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }
  .mandate-option-meta strong {
    font-size: 0.95rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mandate-option-meta span {
    font-size: 0.82rem;
  }
  .mandate-avatar {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    object-fit: cover;
    flex-shrink: 0;
    background: var(--c-bg);
  }
  .mandate-avatar-ph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 1rem;
  }
  .selected-mandate {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    padding: 0.6rem 0.7rem;
    background: var(--c-green-soft);
    border: 1px solid var(--c-green-dark);
    border-radius: 8px;
  }
  .selected-meta {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
    flex: 1;
  }
  .selected-meta span {
    font-size: 0.82rem;
  }
  .btn-link {
    background: none;
    border: none;
    color: var(--c-green-dark);
    text-decoration: underline;
    cursor: pointer;
    padding: 0;
    font-family: inherit;
    font-size: 0.85rem;
  }
</style>
