<script lang="ts">
  // Registration: e-mail + senha + CPF, with client-side CPF check-digit
  // validation. Toggle Cidadão | Político/Candidata. In politician mode the
  // user picks their mandate from a searchable list; on submit we hit
  // /auth/register/politician which enforces `email == mandate.public_email`
  // server-side.
  //
  // 0.17.0: form fields now use ui/Input, ui/Button, ui/Alert, ui/Avatar,
  // ui/Icon. Role picker and mandate picker keep custom layouts but use tokens.
  import {
    register,
    registerPolitician,
    getAllMandates,
    DEFAULT_ORG_ID,
    type MandateDto,
  } from '../../lib/api';
  import { formatCpf, isValidCpf, onlyDigits } from '../../lib/cpf';
  import Input from '../ui/Input.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';

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
    emailValid &&
      passwordValid &&
      cpfValid &&
      (role === 'cidadao' || selectedMandate !== null),
  );

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
    // Register-as-politician só é usado hoje por federais/estaduais. Municipais
    // (~68k) tornam o picker inviável e são cobertos por um flow dedicado.
    const [fed, est] = await Promise.all([
      getAllMandates(DEFAULT_ORG_ID, 5000, 'federal'),
      getAllMandates(DEFAULT_ORG_ID, 5000, 'estadual'),
    ]);
    mandates = [
      ...(fed.ok && fed.data ? fed.data : []),
      ...(est.ok && est.data ? est.data : []),
    ];
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

    const res =
      role === 'politico' && selectedMandate
        ? await registerPolitician(
            email,
            password,
            onlyDigits(cpf),
            selectedMandate.id,
          )
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
      window.location.href =
        role === 'politico' ? '/painel-mandato' : '/bem-vinda';
    } else {
      serverError =
        res.error?.message ??
        'Não foi possível criar sua conta. Tente novamente.';
    }
  }
</script>

<form class="auth-form" onsubmit={submit} novalidate>
  <fieldset class="role-picker" aria-label="Tipo de conta">
    <legend class="role-legend">Você está entrando como</legend>
    <div class="role-tabs">
      <button
        type="button"
        class="role-tab"
        class:active={role === 'cidadao'}
        onclick={() => switchRole('cidadao')}
      >
        <span class="role-ic"><Icon name="users" size={20} /></span>
        <span>
          <strong>Cidadã(o)</strong>
          <span class="role-hint">Propor, votar, cobrar</span>
        </span>
      </button>
      <button
        type="button"
        class="role-tab"
        class:active={role === 'politico'}
        onclick={() => switchRole('politico')}
      >
        <span class="role-ic"><Icon name="mandate" size={20} /></span>
        <span>
          <strong>Político(a) / Candidata(o)</strong>
          <span class="role-hint">Responder, prestar contas</span>
        </span>
      </button>
    </div>
  </fieldset>

  {#if role === 'politico'}
    <div class="field">
      <label for="r-mandate">Seu mandato</label>
      {#if selectedMandate}
        <div class="selected-mandate">
          <Avatar
            src={selectedMandate.avatar_url}
            name={selectedMandate.display_name}
            size="sm"
          />
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
          >
            trocar
          </button>
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
                  <Avatar src={m.avatar_url} name={m.display_name} size="sm" />
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
        Use o e-mail oficial do gabinete cadastrado na Câmara/Senado/TSE — é
        assim que confirmamos que o mandato é seu.
      </p>
    </div>
  {/if}

  <Input
    id="r-email"
    label="E-mail"
    type="email"
    autocomplete="email"
    bind:value={email}
    required
    leading={atIcon}
    error={email.length > 0 && !emailValid
      ? 'Informe um e-mail válido.'
      : undefined}
  />

  <Input
    id="r-password"
    label="Senha"
    type="password"
    autocomplete="new-password"
    bind:value={password}
    required
    leading={lockIcon}
    hint="Mínimo de 8 caracteres."
    error={password.length > 0 && !passwordValid
      ? 'A senha precisa ter pelo menos 8 caracteres.'
      : undefined}
  />

  <Input
    id="r-cpf"
    label="CPF"
    placeholder="000.000.000-00"
    autocomplete="off"
    inputmode="numeric"
    maxlength={14}
    bind:value={cpf}
    oninput={onCpfInput}
    onblur={() => (cpfTouched = true)}
    required
    hint={cpf.length > 0 && cpfValid
      ? '✓ CPF válido.'
      : 'Usado apenas para verificar a sua identidade cívica.'}
    error={(cpfTouched || onlyDigits(cpf).length === 11) &&
    cpf.length > 0 &&
    !cpfValid
      ? 'CPF inválido. Verifique os dígitos.'
      : undefined}
  />

  {#snippet atIcon()}<Icon name="at" size={16} />{/snippet}
  {#snippet lockIcon()}<Icon name="lock" size={16} />{/snippet}

  <Button
    type="submit"
    variant="primary"
    size="lg"
    fullWidth
    loading={busy}
    disabled={!valid}
  >
    {role === 'politico'
      ? 'Criar conta e assumir o mandato'
      : 'Criar minha conta'}
  </Button>

  {#if serverError}
    <div class="err">
      <Alert tone="danger">{serverError}</Alert>
    </div>
  {/if}

  <p class="alt muted">
    Já tem conta? <a href="/entrar">Entrar</a>
  </p>
</form>

<style>
  .auth-form {
    display: block;
  }
  .err {
    margin-top: var(--sp-3);
  }
  .alt {
    margin-top: var(--sp-3);
    text-align: center;
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .alt a {
    color: var(--accent);
    font-weight: var(--fw-medium);
  }
  .role-picker {
    border: none;
    padding: 0;
    margin: 0 0 var(--sp-5);
  }
  .role-legend {
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
    margin-bottom: var(--sp-2);
    padding: 0;
  }
  .role-tabs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-2);
  }
  .role-tab {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    padding: var(--sp-3) var(--sp-4);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    cursor: pointer;
    font-family: inherit;
    text-align: left;
    color: var(--text-1);
    transition:
      background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .role-tab > span:last-child {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .role-tab:hover {
    background: var(--surface-2);
  }
  .role-tab.active {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .role-tab.active .role-ic {
    color: var(--accent);
  }
  .role-ic {
    color: var(--text-3);
    display: inline-flex;
    padding-top: 2px;
  }
  .role-tab strong {
    font-size: var(--fs-sm);
  }
  .role-hint {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  @media (max-width: 480px) {
    .role-tabs {
      grid-template-columns: 1fr;
    }
  }
  .field {
    display: block;
    margin-bottom: var(--sp-4);
  }
  .field > label {
    display: block;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-1);
    color: var(--text-1);
  }
  .mandate-list {
    list-style: none;
    padding: 0;
    margin: var(--sp-1) 0 0;
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    max-height: 260px;
    overflow-y: auto;
    background: var(--surface-1);
    box-shadow: var(--shadow-sm);
  }
  .mandate-option {
    display: flex;
    gap: var(--sp-3);
    align-items: center;
    width: 100%;
    padding: var(--sp-2) var(--sp-3);
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    color: var(--text-1);
    transition: background var(--dur-fast) var(--ease-out);
  }
  .mandate-option:last-child {
    border-bottom: none;
  }
  .mandate-option:hover,
  .mandate-option:focus {
    background: var(--surface-2);
  }
  .mandate-option-meta {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .mandate-option-meta strong {
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mandate-option-meta span {
    font-size: var(--fs-xs);
  }
  .selected-mandate {
    display: flex;
    gap: var(--sp-3);
    align-items: center;
    padding: var(--sp-3) var(--sp-4);
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    border-radius: var(--r-sm);
  }
  .selected-meta {
    display: grid;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .selected-meta strong {
    color: var(--text-1);
    font-size: var(--fs-sm);
  }
  .selected-meta span {
    font-size: var(--fs-xs);
  }
  .btn-link {
    background: none;
    border: none;
    color: var(--accent-strong);
    text-decoration: underline;
    cursor: pointer;
    padding: 0;
    font-family: inherit;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
  }
  .muted {
    color: var(--text-3);
  }
</style>
