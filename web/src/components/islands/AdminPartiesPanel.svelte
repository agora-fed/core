<script lang="ts">
  // /admin/partidos (ÁGORA F1, #58) — atribui Administradores de Partido (escopo
  // nacional) e de Diretório (federal/estadual/municipal) sobre o modelo que já
  // existe no schema (party/party_directory/party_administrator, 0204). Consome a
  // API em inglês /admin/parties (ADR-0013). Gated por party.manage no backend.
  import { onMount } from 'svelte';
  import {
    listParties,
    listDirectories,
    createDirectory,
    listPartyAdministrators,
    assignPartyAdministrator,
    removePartyAdministrator,
    sendBroadcast,
    importContacts,
    type PartyDto,
    type DirectoryDto,
    type PartyAdministratorDto,
  } from '../../lib/api';
  import { UFS } from '../../lib/ufs';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let parties = $state<PartyDto[]>([]);
  let selected = $state<string | null>(null);

  let directories = $state<DirectoryDto[]>([]);
  let administrators = $state<PartyAdministratorDto[]>([]);
  let scopeBusy = $state(false);

  // Form: novo diretório
  let dirEsfera = $state<'federal' | 'estadual' | 'municipal'>('estadual');
  let dirUf = $state('');
  let dirMunicipio = $state('');
  let dirName = $state('');

  // Form: atribuir administrador
  let admHandle = $state('');
  let admRole = $state<'admin' | 'moderador'>('admin');
  let admDirectoryId = $state(''); // '' = nacional

  // Form: broadcast consentido (só diretórios municipais)
  let bcDirId = $state('');
  let bcSubject = $state('');
  let bcBody = $state('');
  let bcQuestions = $state<string[]>(['', '', '']);
  let bcResultLink = $state<string | null>(null);
  let municipalDirs = $derived(directories.filter((d) => d.esfera === 'municipal'));

  // Form: base própria de contatos (F4)
  let ctDirId = $state('');
  let ctBasis = $state('consent');
  let ctText = $state('');
  let ctResult = $state<string | null>(null);

  async function reloadParties() {
    loading = true;
    error = null;
    const res = await listParties();
    loading = false;
    if (!res.success) {
      error = res.error?.message ?? 'Não foi possível carregar (precisa da permissão party.manage).';
      return;
    }
    parties = res.data ?? [];
  }

  async function selectParty(sigla: string) {
    selected = sigla;
    scopeBusy = true;
    const [d, a] = await Promise.all([listDirectories(sigla), listPartyAdministrators(sigla)]);
    scopeBusy = false;
    directories = d.success ? (d.data ?? []) : [];
    administrators = a.success ? (a.data ?? []) : [];
    if (!d.success) toast.error(d.error?.message ?? 'Erro ao carregar diretórios');
    if (!a.success) toast.error(a.error?.message ?? 'Erro ao carregar administradores');
  }

  function directoryLabel(id: string | null): string {
    if (!id) return 'Nacional (todo o partido)';
    const d = directories.find((x) => x.id === id);
    return d ? scopeText(d) : 'Diretório';
  }

  function scopeText(d: DirectoryDto): string {
    if (d.esfera === 'federal') return 'Federal';
    if (d.esfera === 'estadual') return `Estadual · ${d.uf}`;
    return `Municipal · ${d.municipio}/${d.uf}`;
  }

  async function submitDirectory(e: Event) {
    e.preventDefault();
    if (!selected) return;
    const name = dirName.trim();
    if (!name) {
      toast.error('Informe o nome do diretório');
      return;
    }
    const body: { esfera: string; uf?: string; municipio?: string; name: string } = {
      esfera: dirEsfera,
      name,
    };
    if (dirEsfera !== 'federal') body.uf = dirUf;
    if (dirEsfera === 'municipal') body.municipio = dirMunicipio.trim();
    scopeBusy = true;
    const res = await createDirectory(selected, body);
    scopeBusy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível criar o diretório');
      return;
    }
    toast.success('Diretório criado');
    dirName = '';
    dirMunicipio = '';
    await selectParty(selected);
  }

  async function submitAdministrator(e: Event) {
    e.preventDefault();
    if (!selected) return;
    const handle = admHandle.trim();
    if (!handle) {
      toast.error('Informe o handle do cidadão');
      return;
    }
    scopeBusy = true;
    const res = await assignPartyAdministrator(selected, {
      handle,
      role: admRole,
      directory_id: admDirectoryId || undefined,
    });
    scopeBusy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível atribuir');
      return;
    }
    toast.success('Administrador atribuído');
    admHandle = '';
    await selectParty(selected);
    await reloadParties();
  }

  async function remove(id: string) {
    if (!selected) return;
    scopeBusy = true;
    const res = await removePartyAdministrator(selected, id);
    scopeBusy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível remover');
      return;
    }
    toast.success('Administrador removido');
    await selectParty(selected);
    await reloadParties();
  }

  async function submitBroadcast(e: Event) {
    e.preventDefault();
    if (!selected) return;
    if (!bcDirId) return toast.error('Escolha o diretório municipal');
    const subject = bcSubject.trim();
    const body = bcBody.trim();
    if (!subject || !body) return toast.error('Preencha assunto e mensagem');
    const questions = bcQuestions.map((q) => q.trim()).filter(Boolean);
    scopeBusy = true;
    bcResultLink = null;
    const res = await sendBroadcast(selected, bcDirId, { subject, body, questions });
    scopeBusy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível enviar');
    toast.success(`Enviado a ${res.data?.recipients ?? 0} destinatário(s) consentido(s)`);
    if (res.data?.consultation_id) {
      bcResultLink = `/consulta/?id=${res.data.consultation_id}`;
    }
    bcSubject = '';
    bcBody = '';
    bcQuestions = ['', '', ''];
  }

  async function submitContacts(e: Event) {
    e.preventDefault();
    if (!selected) return;
    if (!ctDirId) return toast.error('Escolha o diretório');
    const tokens = ctText
      .split(/[\s,;]+/)
      .map((t) => t.trim())
      .filter((t) => t.includes('@'));
    if (tokens.length === 0) return toast.error('Cole ao menos um e-mail');
    scopeBusy = true;
    ctResult = null;
    const res = await importContacts(selected, ctDirId, {
      legal_basis: ctBasis,
      contacts: tokens.map((email) => ({ email })),
    });
    scopeBusy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível importar');
    const d = res.data;
    ctResult = `${d?.inserted ?? 0} importados · ${d?.matched ?? 0} casados na base central · ${d?.duplicates ?? 0} duplicados · ${d?.invalid ?? 0} inválidos`;
    toast.success('Contatos importados');
    ctText = '';
  }

  onMount(reloadParties);
</script>

<div class="wrap">
  {#if loading}
    <p class="muted">Carregando partidos…</p>
  {:else if error}
    <p class="err">{error}</p>
  {:else}
    <div class="cols">
      <!-- Lista de partidos -->
      <aside class="parties">
        <h2>Partidos <span class="count">{parties.length}</span></h2>
        <ul>
          {#each parties as p (p.sigla)}
            <li>
              <button
                class:active={selected === p.sigla}
                onclick={() => selectParty(p.sigla)}
              >
                <span class="sigla">{p.sigla}</span>
                <span class="pname">{p.name}</span>
                <span class="badges">{p.directory_count} dir · {p.administrator_count} adm</span>
              </button>
            </li>
          {/each}
        </ul>
      </aside>

      <!-- Detalhe do partido -->
      <section class="detail">
        {#if !selected}
          <p class="muted">Selecione um partido à esquerda para gerir diretórios e administradores.</p>
        {:else}
          <h2 class="dhead">{selected}</h2>

          <!-- Diretórios -->
          <h3>Diretórios</h3>
          {#if directories.length === 0}
            <p class="muted small">Nenhum diretório ainda.</p>
          {:else}
            <ul class="rows">
              {#each directories as d (d.id)}
                <li><span class="scope">{scopeText(d)}</span><span class="rname">{d.name}</span></li>
              {/each}
            </ul>
          {/if}
          <form class="mini" onsubmit={submitDirectory}>
            <select bind:value={dirEsfera} class="input">
              <option value="federal">Federal</option>
              <option value="estadual">Estadual</option>
              <option value="municipal">Municipal</option>
            </select>
            {#if dirEsfera !== 'federal'}
              <select bind:value={dirUf} class="input" required>
                <option value="" disabled>UF</option>
                {#each UFS as uf (uf.code)}
                  <option value={uf.code}>{uf.code}</option>
                {/each}
              </select>
            {/if}
            {#if dirEsfera === 'municipal'}
              <input bind:value={dirMunicipio} class="input" placeholder="Município" required />
            {/if}
            <input bind:value={dirName} class="input grow" placeholder="Nome do diretório" required />
            <button class="btn" disabled={scopeBusy}>Criar diretório</button>
          </form>

          <hr />

          <!-- Administradores -->
          <h3>Administradores</h3>
          {#if administrators.length === 0}
            <p class="muted small">Nenhum administrador ainda.</p>
          {:else}
            <ul class="rows">
              {#each administrators as a (a.id)}
                <li>
                  <span class="scope">{a.role === 'admin' ? 'Admin' : 'Moderador'}</span>
                  <span class="rname">@{a.handle ?? a.citizen_id.slice(0, 8)}</span>
                  <span class="scope2">{directoryLabel(a.directory_id)}</span>
                  <button class="rm" onclick={() => remove(a.id)} disabled={scopeBusy}>Remover</button>
                </li>
              {/each}
            </ul>
          {/if}
          <form class="mini" onsubmit={submitAdministrator}>
            <input bind:value={admHandle} class="input grow" placeholder="@handle do cidadão" required />
            <select bind:value={admRole} class="input">
              <option value="admin">Admin</option>
              <option value="moderador">Moderador</option>
            </select>
            <select bind:value={admDirectoryId} class="input">
              <option value="">Nacional (todo o partido)</option>
              {#each directories as d (d.id)}
                <option value={d.id}>{scopeText(d)}</option>
              {/each}
            </select>
            <button class="btn" disabled={scopeBusy}>Atribuir</button>
          </form>

          {#if municipalDirs.length > 0}
            <hr />
            <h3>Comunicado (broadcast consentido)</h3>
            <p class="muted small">
              Envia por e-mail <strong>só a quem autorizou</strong> receber campanha e reside no
              município do diretório. A lista nunca é exposta; há opt-out no rodapé. Limite: 1 envio
              a cada 24h por diretório.
            </p>
            <form class="mini col" onsubmit={submitBroadcast}>
              <select bind:value={bcDirId} class="input" required>
                <option value="" disabled>Diretório municipal</option>
                {#each municipalDirs as d (d.id)}
                  <option value={d.id}>{scopeText(d)}</option>
                {/each}
              </select>
              <input bind:value={bcSubject} class="input" placeholder="Assunto" required />
              <textarea bind:value={bcBody} class="input area" placeholder="Mensagem" rows="4" required
              ></textarea>
              <span class="q-label"
                >Micro-consulta (opcional) — até 3 perguntas (concordo/neutro/discordo):</span
              >
              {#each bcQuestions as _q, i (i)}
                <input bind:value={bcQuestions[i]} class="input" placeholder={`Pergunta ${i + 1}`} />
              {/each}
              <button class="btn" disabled={scopeBusy}>Enviar comunicado</button>
            </form>
            {#if bcResultLink}
              <p class="muted small">
                Micro-consulta criada: <a href={bcResultLink}>ver e responder</a> — o link também foi
                no e-mail à base consentida.
              </p>
            {/if}
          {/if}

          {#if directories.length > 0}
            <hr />
            <h3>Base própria de contatos</h3>
            <p class="muted small">
              O diretório sobe <strong>seus próprios</strong> contatos (base legal declarada). Fica
              <strong>isolada</strong> e apagável em bloco (LGPD); verificamos contra a base central
              (casa por e-mail e enriquece o domicílio). Cole os e-mails, um por linha.
            </p>
            <form class="mini col" onsubmit={submitContacts}>
              <select bind:value={ctDirId} class="input" required>
                <option value="" disabled>Diretório</option>
                {#each directories as d (d.id)}
                  <option value={d.id}>{scopeText(d)}</option>
                {/each}
              </select>
              <select bind:value={ctBasis} class="input">
                <option value="consent">Base legal: consentimento</option>
                <option value="legitimate_interest">Base legal: legítimo interesse</option>
                <option value="contract">Base legal: execução de contrato</option>
              </select>
              <textarea
                bind:value={ctText}
                class="input area"
                placeholder="um e-mail por linha"
                rows="4"
                required
              ></textarea>
              <button class="btn" disabled={scopeBusy}>Importar contatos</button>
            </form>
            {#if ctResult}
              <p class="muted small">{ctResult}</p>
            {/if}
          {/if}
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .cols { display: grid; grid-template-columns: 20rem 1fr; gap: var(--sp-5); align-items: start; }
  @media (max-width: 720px) { .cols { grid-template-columns: 1fr; } }
  h2 { font-size: var(--fs-md); margin: 0 0 var(--sp-3); }
  .count { color: var(--text-3); font-weight: var(--fw-regular); }
  .parties ul, .rows { list-style: none; margin: 0; padding: 0; }
  .parties li { margin-bottom: 4px; }
  .parties button { width: 100%; text-align: left; display: grid; gap: 2px; padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); cursor: pointer; }
  .parties button.active { border-color: var(--accent); background: var(--surface-2); }
  .sigla { font-weight: var(--fw-semibold); color: var(--text-1); }
  .pname { font-size: var(--fs-sm); color: var(--text-2); }
  .badges { font-size: var(--fs-xs); color: var(--text-3); }
  .dhead { font-size: var(--fs-lg); }
  h3 { font-size: var(--fs-md); margin: var(--sp-4) 0 var(--sp-2); color: var(--text-1); }
  .rows li { display: flex; align-items: center; gap: var(--sp-3); padding: var(--sp-2) 0; border-top: 1px solid var(--border-subtle); }
  .scope { font-weight: var(--fw-semibold); color: var(--accent-strong); min-width: 8rem; }
  .scope2 { color: var(--text-3); font-size: var(--fs-sm); margin-left: auto; }
  .rname { color: var(--text-1); }
  .rm { margin-left: auto; border: 1px solid var(--border-subtle); background: var(--surface-1); border-radius: var(--r-sm); padding: 2px 10px; font-size: var(--fs-sm); cursor: pointer; color: var(--danger, #b91c1c); }
  .mini { display: flex; flex-wrap: wrap; gap: var(--sp-2); margin-top: var(--sp-3); }
  .mini.col { flex-direction: column; align-items: stretch; max-width: 32rem; }
  .area { resize: vertical; font-family: inherit; }
  .q-label { font-size: var(--fs-sm); color: var(--text-2); margin-top: var(--sp-1); }
  .input { padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); font-size: var(--fs-sm); }
  .grow { flex: 1; min-width: 10rem; }
  .btn { padding: var(--sp-2) var(--sp-4); border-radius: var(--r-sm); border: 1px solid var(--accent); background: var(--accent); color: #fff; font-weight: var(--fw-semibold); font-size: var(--fs-sm); cursor: pointer; }
  .btn:disabled { opacity: 0.6; cursor: default; }
  hr { border: none; border-top: 1px solid var(--border-subtle); margin: var(--sp-4) 0; }
  .muted { color: var(--text-3); }
  .small { font-size: var(--fs-sm); }
  .err { color: #b91c1c; }
</style>
