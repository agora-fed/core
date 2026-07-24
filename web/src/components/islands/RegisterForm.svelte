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
    registerCandidate,
    registerResend,
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

  type Role = 'cidadao' | 'politico' | 'candidato';

  let role = $state<Role>('cidadao');
  let email = $state('');
  let password = $state('');
  let cpf = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let cpfTouched = $state(false);
  // 0.25.0-fediverso: /auth/register agora responde 202 e dispara e-mail com
  // link de confirmação. Trocamos o formulário por uma tela "verifique seu
  // e-mail" nesse caso, ao invés de já redirecionar pra /bem-vinda.
  let verificationSentTo = $state<string | null>(null);
  let resendState = $state<'idle' | 'busy' | 'sent'>('idle');

  async function resendLink() {
    if (!verificationSentTo || resendState === 'busy') return;
    resendState = 'busy';
    await registerResend(verificationSentTo);
    resendState = 'sent';
    // Volta ao idle depois de 6s pra permitir uma nova tentativa.
    window.setTimeout(() => (resendState = 'idle'), 6000);
  }

  // Politician-mode state.
  let mandateSearch = $state('');
  let selectedMandate = $state<MandateDto | null>(null);
  let mandates = $state<MandateDto[]>([]);
  let mandatesLoaded = $state(false);
  let mandateListOpen = $state(false);

  // Candidate-mode state (0.36.0 — candidatura auto-declarada, sem mandato).
  const OFFICES = [
    { value: 'vereador', label: 'Vereador(a)', sphere: 'municipal' },
    { value: 'prefeito', label: 'Prefeito(a)', sphere: 'municipal' },
    { value: 'vice_prefeito', label: 'Vice-prefeito(a)', sphere: 'municipal' },
    { value: 'deputado_estadual', label: 'Deputado(a) estadual', sphere: 'estadual' },
    { value: 'governador', label: 'Governador(a)', sphere: 'estadual' },
    { value: 'deputado_federal', label: 'Deputado(a) federal', sphere: 'federal' },
    { value: 'senador', label: 'Senador(a)', sphere: 'federal' },
    { value: 'presidente', label: 'Presidente', sphere: 'federal' },
  ] as const;
  const UFS = [
    'AC','AL','AM','AP','BA','CE','DF','ES','GO','MA','MG','MS','MT','PA',
    'PB','PE','PI','PR','RJ','RN','RO','RR','RS','SC','SE','SP','TO',
  ] as const;
  let candName = $state('');
  let candOffice = $state('');
  let candUf = $state('');
  let candMunicipio = $state('');
  let candParty = $state('');
  let candNumber = $state('');

  let candSphere = $derived(
    OFFICES.find((o) => o.value === candOffice)?.sphere ?? null,
  );
  let candValid = $derived(
    candName.trim().length >= 3 &&
      candOffice !== '' &&
      (candOffice === 'presidente' || candUf !== '') &&
      (candSphere !== 'municipal' || candMunicipio.trim().length > 0) &&
      candParty.trim().length >= 2 &&
      (candNumber.trim() === '' || /^\d{2,5}$/.test(candNumber.trim())),
  );

  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  let passwordValid = $derived(password.length >= 8);
  let cpfValid = $derived(isValidCpf(cpf));
  let valid = $derived(
    emailValid &&
      passwordValid &&
      cpfValid &&
      (role === 'cidadao' ||
        (role === 'politico' && selectedMandate !== null) ||
        (role === 'candidato' && candValid)),
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
        : role === 'candidato'
          ? await registerCandidate(email, password, onlyDigits(cpf), {
              display_name: candName.trim(),
              office: candOffice,
              uf: candOffice === 'presidente' && !candUf ? undefined : candUf,
              municipio:
                candSphere === 'municipal' ? candMunicipio.trim() : undefined,
              party_sigla: candParty.trim().toUpperCase(),
              number: candNumber.trim() || undefined,
            })
          : await register(email, password, onlyDigits(cpf));
    busy = false;

    if (res.success) {
      // Não há mais citizen_id nem sessão aqui — só a confirmação de que
      // o e-mail saiu. O clique no link cai em /confirmar-conta e é lá que
      // a sessão é emitida e o localStorage é populado.
      verificationSentTo = res.data?.email ?? email.trim().toLowerCase();
    } else {
      serverError =
        res.error?.message ??
        'Não foi possível criar sua conta. Tente novamente.';
    }
  }
</script>

{#if verificationSentTo}
  <section class="sent" aria-live="polite">
    <div class="sent-ic" aria-hidden="true">
      <Icon name="at" size={28} />
    </div>
    <h2>Confirme seu e-mail pra ativar a conta</h2>
    <p>
      Enviamos um link de verificação pra <strong>{verificationSentTo}</strong>.
      Abra a mensagem em até 24 horas e clique no link — só depois disso a
      conta é criada e você entra automaticamente.
    </p>
    <p class="hint muted">
      Não achou? Confira spam / lixeira. Se ainda não aparecer, peça outro
      envio.
    </p>
    <div class="sent-cta">
      <Button
        type="button"
        variant="primary"
        size="md"
        loading={resendState === 'busy'}
        disabled={resendState !== 'idle'}
        onclick={resendLink}
      >
        {resendState === 'sent' ? 'Enviado ✓' : 'Reenviar link'}
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="md"
        onclick={() => {
          verificationSentTo = null;
          resendState = 'idle';
        }}
      >
        Usar outro e-mail
      </Button>
    </div>
  </section>
{:else}
<section class="rules" aria-label="Regras da conta">
  <h2>Antes de criar sua conta</h2>
  <ul>
    <li>
      <strong>Até 3 000 caracteres por publicação</strong> — foco em
      argumentar, não em spammar.
    </li>
    <li>
      <strong>Uma publicação a cada 15 minutos</strong> — pra debate saudável,
      não enxurrada.
    </li>
    <li>
      <strong>Só cidadã(o) verificada(o) vota em pauta urgente.</strong>
      Título de eleitor confirmado sinaliza voz brasileira.
    </li>
    <li>
      <strong>Enquetes fediversadas, mas votação restrita a esta instância.</strong>
      Perfis de outras instâncias veem, mas não computam voto.
    </li>
  </ul>
</section>

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
          <strong>Político(a) com mandato</strong>
          <span class="role-hint">Responder, prestar contas</span>
        </span>
      </button>
      <button
        type="button"
        class="role-tab"
        class:active={role === 'candidato'}
        onclick={() => switchRole('candidato')}
      >
        <span class="role-ic"><Icon name="ballot" size={20} /></span>
        <span>
          <strong>Candidato(a) 2026</strong>
          <span class="role-hint">Ainda sem mandato</span>
        </span>
      </button>
    </div>
  </fieldset>

  {#if role === 'candidato'}
    <div class="field">
      <Input
        id="r-cand-name"
        label="Nome de urna"
        type="text"
        bind:value={candName}
        required
        error={candName.length > 0 && candName.trim().length < 3
          ? 'Mínimo de 3 caracteres.'
          : undefined}
      />
      <div class="cand-grid">
        <label class="cand-select">
          <span>Cargo pretendido</span>
          <select class="input" bind:value={candOffice} required>
            <option value="" disabled>Selecione…</option>
            {#each OFFICES as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </select>
        </label>
        {#if candOffice !== 'presidente'}
          <label class="cand-select">
            <span>UF</span>
            <select class="input" bind:value={candUf} required>
              <option value="" disabled>UF…</option>
              {#each UFS as uf (uf)}
                <option value={uf}>{uf}</option>
              {/each}
            </select>
          </label>
        {/if}
      </div>
      {#if candSphere === 'municipal'}
        <Input
          id="r-cand-mun"
          label="Município"
          type="text"
          bind:value={candMunicipio}
          required
        />
      {/if}
      <div class="cand-grid">
        <Input
          id="r-cand-party"
          label="Partido (sigla)"
          type="text"
          bind:value={candParty}
          required
          error={candParty.length > 0 && candParty.trim().length < 2
            ? 'Sigla curta demais.'
            : undefined}
        />
        <Input
          id="r-cand-number"
          label="Número (opcional)"
          type="text"
          inputmode="numeric"
          bind:value={candNumber}
          error={candNumber.trim() !== '' && !/^\d{2,5}$/.test(candNumber.trim())
            ? 'De 2 a 5 dígitos.'
            : undefined}
        />
      </div>
      <p class="hint muted">
        Sua candidatura entra como <strong>autodeclarada</strong>: as
        ferramentas de campanha e financiamento destravam agora, e o selo de
        verificação vem depois — por atestação do seu partido, por um mandato
        já verificado ou pelo registro oficial no TSE.
      </p>
    </div>
  {/if}

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
{/if}

<style>
  .sent {
    display: grid;
    gap: var(--sp-3);
    padding: var(--sp-5);
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    border-radius: var(--r-base);
    text-align: center;
  }
  .sent-ic {
    display: inline-flex;
    justify-content: center;
    color: var(--accent-strong);
  }
  .sent h2 {
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .sent p {
    margin: 0;
    line-height: var(--lh-relaxed);
    color: var(--text-2);
  }
  .sent .hint {
    font-size: var(--fs-sm);
  }
  .sent-cta {
    display: flex;
    gap: var(--sp-2);
    justify-content: center;
    flex-wrap: wrap;
  }
  .rules {
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    padding: var(--sp-4);
    margin-bottom: var(--sp-4);
  }
  .rules h2 {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-base);
    color: var(--text-1);
  }
  .rules ul {
    margin: 0;
    padding-left: var(--sp-4);
    display: grid;
    gap: var(--sp-2);
    color: var(--text-2);
    font-size: var(--fs-sm);
    line-height: var(--lh-snug);
  }
  .rules strong {
    color: var(--text-1);
  }
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
  /* 3 papéis num card de 32rem: linhas empilhadas (lado a lado estourava). */
  .role-tabs {
    display: grid;
    grid-template-columns: 1fr;
    gap: var(--sp-2);
  }
  .role-tab span:last-child {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 var(--sp-2);
  }
  /* minmax(0,…): inputs têm min-width intrínseco e estouram colunas fr puras. */
  .cand-grid {
    display: grid;
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
    gap: var(--sp-3);
    margin-top: var(--sp-3);
  }
  .cand-grid :global(input) {
    min-width: 0;
    width: 100%;
  }
  .cand-select {
    display: block;
  }
  .cand-select > span {
    display: block;
    font-weight: var(--fw-medium);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-1);
  }
  .cand-select select.input {
    width: 100%;
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
