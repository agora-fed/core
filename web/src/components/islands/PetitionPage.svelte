<script lang="ts">
  // /abaixo-assinado/?id=<uuid>: the text, the progress, and the sign form.
  // Members sign with one click. Visitors give name and e-mail, pass the proof-of-work
  // check, and confirm through the link the server (or the identity provider) e-mails.
  import { onMount } from 'svelte';
  import {
    getPetition,
    humanCheck,
    petitionSettings,
    progress,
    setPetitionStatus,
    signAsMember,
    signAsVisitor,
    type PetitionDetail,
    type PetitionSettings,
    type PetitionStatus,
  } from '../../lib/petitions';
  import { SITE_NAME } from '../../lib/site';
  import { toast } from '../../lib/toasts';

  const siteName = SITE_NAME;

  let id = $state('');
  let loading = $state(true);
  let missing = $state(false);
  let signedIn = $state(false);
  let petition = $state<PetitionDetail | null>(null);
  let settings = $state<PetitionSettings | null>(null);
  let busy = $state(false);
  let checking = $state(0);
  let sentTo = $state<string | null>(null);
  let accountExists = $state(false);
  let form = $state({ name: '', email: '', city: '', show_name: true, consent: false });
  let memberForm = $state({ city: '', show_name: true });

  async function refresh() {
    const res = await getPetition(id);
    if (!res.success || !res.data) {
      missing = true;
      petition = null;
      return;
    }
    petition = res.data;
  }

  async function signMember() {
    busy = true;
    const res = await signAsMember(id, { show_name: memberForm.show_name, city: memberForm.city || undefined });
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível assinar.');
      return;
    }
    toast.success('Assinatura registrada. Obrigado!');
    await refresh();
  }

  async function signVisitor(event: SubmitEvent) {
    event.preventDefault();
    accountExists = false;
    busy = true;
    checking = 0.01;
    const altcha = await humanCheck((share) => (checking = Math.max(0.01, share)));
    checking = 0;
    if (!altcha) {
      busy = false;
      toast.error('A verificação anti-robô não terminou. Recarregue a página e tente de novo.');
      return;
    }
    const res = await signAsVisitor(id, {
      name: form.name,
      email: form.email,
      city: form.city || undefined,
      show_name: form.show_name,
      consent: form.consent,
      altcha,
    });
    busy = false;
    if (res.success) {
      sentTo = form.email.trim();
      return;
    }
    if (res.error?.code === 'account_exists') {
      accountExists = true;
      return;
    }
    toast.error(res.error?.message ?? 'Não foi possível enviar sua assinatura.');
  }

  async function changeStatus(status: PetitionStatus, question: string) {
    if (!confirm(question)) return;
    const res = await setPetitionStatus(id, status);
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível alterar.');
      return;
    }
    await refresh();
  }

  async function share() {
    const url = window.location.href;
    try {
      if (navigator.share) {
        await navigator.share({ title: petition?.title, url });
      } else {
        await navigator.clipboard.writeText(url);
        toast.success('Link copiado.');
      }
    } catch {
      /* the person closed the share sheet */
    }
  }

  onMount(async () => {
    id = new URLSearchParams(window.location.search).get('id') ?? '';
    try {
      signedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      signedIn = false;
    }
    if (!id) {
      missing = true;
      loading = false;
      return;
    }
    const [, s] = await Promise.all([refresh(), petitionSettings()]);
    settings = s.success ? s.data : null;
    loading = false;
  });

  const nextHere = $derived(`/entrar/?next=${encodeURIComponent(`/abaixo-assinado/?id=${id}`)}`);
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if missing || !petition}
  <h1>Abaixo-assinado não encontrado</h1>
  <p><a href="/abaixo-assinados">Ver todos os abaixo-assinados</a></p>
{:else}
  {@const p = petition}
  <p><a href="/abaixo-assinados">← Abaixo-assinados</a></p>
  <h1>{p.title}</h1>
  <p class="muted small">
    {#if p.author_name}Criado por {p.author_name} · {/if}
    {new Date(p.created_at).toLocaleDateString('pt-BR')}
    {#if p.status === 'closed'} · <strong>encerrado</strong>{/if}
    {#if p.status === 'hidden'} · <strong>oculto pela administração</strong>{/if}
  </p>

  <section class="card tally" aria-label="Assinaturas">
    <p class="count"><strong>{p.signature_count.toLocaleString('pt-BR')}</strong> de {p.threshold.toLocaleString('pt-BR')} assinaturas</p>
    <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={progress(p.signature_count, p.threshold)}>
      <span style={`width:${progress(p.signature_count, p.threshold)}%`}></span>
    </div>
    {#if p.reached}<p class="reached">Meta alcançada!</p>{/if}
    <button class="btn" onclick={share}>Compartilhar</button>
  </section>

  <article class="body">{p.body}</article>

  <section class="card sign" aria-labelledby="sign-title">
    <h2 id="sign-title">Assine</h2>
    {#if p.status !== 'open'}
      <p class="muted">Este abaixo-assinado não recebe mais assinaturas.</p>
    {:else if p.signed_by_me}
      <p><strong>Você já assinou.</strong> Ajude compartilhando o link.</p>
    {:else if signedIn}
      <label class="check"><input type="checkbox" bind:checked={memberForm.show_name} /> Mostrar meu nome na lista pública</label>
      <label>Cidade (opcional) <input maxlength="80" bind:value={memberForm.city} /></label>
      <button class="btn btn-primary" disabled={busy} onclick={signMember}>Assinar</button>
    {:else if sentTo}
      <p><strong>Falta só um passo.</strong> Enviamos um e-mail para <strong>{sentTo}</strong>.
        Abra o link da mensagem para confirmar sua assinatura{settings?.pending_ttl_hours ? ` (vale por ${settings.pending_ttl_hours} horas)` : ''}.
        Ela só conta depois disso.</p>
      <p class="muted small">Não chegou? Olhe a caixa de spam. Você pode pedir de novo daqui a um minuto.</p>
    {:else if !settings?.anonymous_signing}
      <p>Entre na sua conta para assinar.</p>
      <a class="btn btn-primary" href={nextHere}>Entrar</a>
    {:else}
      <p class="muted small">Já tem conta? <a href={nextHere}>Entre</a> e assine com um clique.</p>
      <form class="form" onsubmit={signVisitor}>
        <label>Nome <input required minlength="2" maxlength="80" autocomplete="name" bind:value={form.name} /></label>
        <label>E-mail <input required type="email" maxlength="254" autocomplete="email" bind:value={form.email} /></label>
        <label>Cidade (opcional) <input maxlength="80" autocomplete="address-level2" bind:value={form.city} /></label>
        <label class="check"><input type="checkbox" bind:checked={form.show_name} /> Mostrar meu nome na lista pública</label>
        <label class="check">
          <input type="checkbox" required bind:checked={form.consent} />
          <span>
            Concordo com o uso destes dados para registrar minha assinatura.
            {#if settings.creates_sso_account}
              Ao confirmar o e-mail, uma conta em {siteName} e no login único da comunidade será criada com meu
              nome e e-mail. Depois eu recebo um link para escolher uma senha.
            {:else}
              Ao confirmar o e-mail, uma conta em {siteName} será criada com meu nome e e-mail.
            {/if}
            Veja a <a href="/privacidade" target="_blank" rel="noopener">política de privacidade</a>.
          </span>
        </label>
        {#if accountExists}
          <p class="notice">Este e-mail já tem uma conta. <a href={nextHere}>Entre</a> para assinar com um clique.</p>
        {/if}
        <button class="btn btn-primary" disabled={busy}>
          {#if checking > 0}Verificando que você é uma pessoa… {Math.round(checking * 100)}%{:else}Assinar{/if}
        </button>
        <p class="muted small">Para evitar robôs, seu navegador resolve um pequeno cálculo antes de enviar. Nenhum serviço externo é usado.</p>
      </form>
    {/if}
  </section>

  {#if p.signers.length > 0}
    <section aria-labelledby="signers-title">
      <h2 id="signers-title">Quem assinou</h2>
      <ul class="signers">
        {#each p.signers as s (s.signed_at + s.name)}
          <li>{s.name}{#if s.city} <span class="muted">· {s.city}</span>{/if}</li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if p.can_close}
    <section class="manage">
      {#if p.status === 'open'}
        <button class="btn" onclick={() => changeStatus('closed', 'Encerrar a coleta de assinaturas?')}>Encerrar</button>
      {:else if p.status === 'closed'}
        <button class="btn" onclick={() => changeStatus('open', 'Reabrir a coleta de assinaturas?')}>Reabrir</button>
      {/if}
      {#if p.is_admin && p.status !== 'hidden'}
        <button class="btn btn-danger" onclick={() => changeStatus('hidden', 'Ocultar este abaixo-assinado do público?')}>Ocultar</button>
      {:else if p.is_admin}
        <button class="btn" onclick={() => changeStatus('closed', 'Mostrar de novo, como encerrado?')}>Mostrar de novo</button>
      {/if}
    </section>
  {/if}
{/if}

<style>
  .tally { display: grid; gap: 0.5rem; margin: 1rem 0; }
  .count { margin: 0; font-size: 1.1rem; }
  .bar { height: 0.75rem; background: var(--surface-2); border-radius: 999px; overflow: hidden; }
  .bar span { display: block; height: 100%; background: var(--accent); }
  .reached { margin: 0; color: var(--success); font-weight: 700; }
  .body { white-space: pre-line; line-height: 1.6; margin: 1.5rem 0; }
  .sign { display: grid; gap: 0.75rem; }
  .form { display: grid; gap: 0.75rem; }
  .form label, .sign > label { display: grid; gap: 0.25rem; font-weight: 600; }
  .check { display: flex !important; gap: 0.5rem; align-items: flex-start; font-weight: 400 !important; }
  .notice { background: var(--warning-soft); padding: 0.6rem 0.8rem; border-radius: 0.5rem; }
  .signers { list-style: none; padding: 0; columns: 2; }
  .manage { display: flex; gap: 0.5rem; margin-top: 2rem; }
  .small { font-size: 0.85rem; }
</style>
