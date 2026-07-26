<script lang="ts">
  // /configuracoes#identidade — vincula o título de eleitor à conta.
  //
  // Estado: NULL (nunca cadastrado) | unverified | validated | verified.
  // O POST valida algoritmicamente TSE (dígitos verificadores) e grava
  // titulo_status='validated'. A promoção pra 'verified' (cross-check com
  // TSE dados abertos futuros) fica pra uma fatia posterior.
  //
  // Regra explicada acima do form: só quem tem 'validated' ou 'verified'
  // consegue votar em pauta urgente (Fatia D — separa participação civil
  // de decisão vinculante).
  import { onMount } from 'svelte';
  import { getTituloEleitor, submitTituloEleitor } from '../../lib/api';
  import Input from '../ui/Input.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';

  type Status = 'unverified' | 'validated' | 'verified' | null;

  let titulo = $state('');
  let zona = $state('');
  let secao = $state('');
  let busy = $state(false);
  let loading = $state(true);
  let status = $state<Status>(null);
  let last4 = $state<string | null>(null);
  let savedZona = $state<string | null>(null);
  let savedSecao = $state<string | null>(null);
  let serverError = $state<string | null>(null);
  let justSaved = $state(false);

  // Só dígitos, e pelo menos 12 pra habilitar o submit.
  const onlyDigits = (s: string) => s.replace(/\D/g, '');
  let digits = $derived(onlyDigits(titulo));
  let valid = $derived(digits.length === 12);
  // Zona/seção: até 4 dígitos cada, opcionais.
  let zonaDigits = $derived(onlyDigits(zona).slice(0, 4));
  let secaoDigits = $derived(onlyDigits(secao).slice(0, 4));
  // Com título já vinculado dá pra salvar só zona/seção alteradas.
  let zonaSecaoChanged = $derived(
    zonaDigits !== (savedZona ?? '') || secaoDigits !== (savedSecao ?? ''),
  );
  let canSubmit = $derived(valid || (status !== null && zonaSecaoChanged));

  function onInput(event: Event) {
    const el = event.target as HTMLInputElement;
    // Máscara 4 4 4 pra facilitar leitura.
    const d = onlyDigits(el.value).slice(0, 12);
    titulo = d.length <= 4 ? d : d.length <= 8 ? `${d.slice(0, 4)} ${d.slice(4)}` : `${d.slice(0, 4)} ${d.slice(4, 8)} ${d.slice(8)}`;
  }

  async function refresh() {
    loading = true;
    const res = await getTituloEleitor();
    loading = false;
    if (res.ok && res.data) {
      status = res.data.titulo_status;
      last4 = res.data.titulo_last4;
      savedZona = res.data.titulo_zona;
      savedSecao = res.data.titulo_secao;
      zona = res.data.titulo_zona ?? '';
      secao = res.data.titulo_secao ?? '';
    }
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!canSubmit || busy) return;
    serverError = null;
    busy = true;
    // Título vazio + já vinculado → o backend atualiza só zona/seção.
    const res = await submitTituloEleitor(
      valid ? digits : '',
      zonaDigits,
      secaoDigits,
    );
    busy = false;
    if (res.success && res.data) {
      status = res.data.titulo_status as Status;
      last4 = res.data.titulo_last4;
      savedZona = res.data.titulo_zona;
      savedSecao = res.data.titulo_secao;
      zona = res.data.titulo_zona ?? '';
      secao = res.data.titulo_secao ?? '';
      titulo = '';
      justSaved = true;
      window.setTimeout(() => (justSaved = false), 4000);
    } else {
      serverError =
        res.error?.message ??
        'Não conseguimos validar o título. Confira os dígitos e tente de novo.';
    }
  }

  onMount(refresh);

  const statusLabel: Record<Exclude<Status, null>, { text: string; tone: string }> = {
    unverified: { text: 'Enviado — aguardando validação', tone: 'warn' },
    validated: { text: 'Validado (dígitos TSE OK)', tone: 'ok' },
    verified: { text: 'Verificado (cross-check TSE)', tone: 'ok' },
  };
</script>

<Card>
  <h3 class="head">
    <Icon name="badge" size={18} />
    Título de eleitor
  </h3>
  <p class="muted lede">
    Vincule seu título pra sinalizar cidadania política brasileira. Só cidadã(o)s com
    título <strong>validado</strong> ou <strong>verificado</strong> votam em pauta urgente —
    separação entre participação civil (todo mundo) e decisão vinculante (cidadã(o)
    verificada(o) apta a votar no Brasil real).
  </p>

  {#if loading}
    <p class="muted small">Carregando status…</p>
  {:else if status}
    <div class="status" data-tone={statusLabel[status].tone}>
      <div>
        <strong>{statusLabel[status].text}</strong>
        {#if last4}
          <span class="muted small">Final ••••{last4}</span>
        {/if}
        {#if savedZona || savedSecao}
          <span class="muted small">
            {#if savedZona}Zona {savedZona}{/if}{#if savedZona && savedSecao} · {/if}{#if savedSecao}Seção {savedSecao}{/if}
          </span>
        {/if}
      </div>
      {#if justSaved}
        <span class="badge ok">salvo</span>
      {/if}
    </div>
    <p class="muted small">
      Pra vincular outro título, digite os 12 dígitos abaixo — o novo substitui o atual.
    </p>
  {:else}
    <p class="muted small">
      Você ainda não vinculou nenhum título. Digite os 12 dígitos abaixo (com ou sem espaços).
    </p>
  {/if}

  <form onsubmit={submit} novalidate class="form">
    <Input
      id="titulo-eleitor"
      label="Número do título"
      placeholder="0000 0000 0000"
      inputmode="numeric"
      autocomplete="off"
      maxlength={14}
      bind:value={titulo}
      oninput={onInput}
      hint={valid
        ? '✓ 12 dígitos prontos pra validar.'
        : `${digits.length}/12 dígitos.`}
      required
    />
    <div class="zona-secao">
      <Input
        id="titulo-zona"
        label="Zona"
        placeholder="ex.: 123"
        inputmode="numeric"
        autocomplete="off"
        maxlength={4}
        bind:value={zona}
        hint="Consta no título (opcional)."
      />
      <Input
        id="titulo-secao"
        label="Seção"
        placeholder="ex.: 45"
        inputmode="numeric"
        autocomplete="off"
        maxlength={4}
        bind:value={secao}
        hint="Consta no título (opcional)."
      />
    </div>
    <Button
      type="submit"
      variant="primary"
      size="md"
      loading={busy}
      disabled={!canSubmit}
    >
      {valid
        ? status
          ? 'Trocar título'
          : 'Vincular título'
        : 'Salvar zona e seção'}
    </Button>
  </form>

  {#if serverError}
    <div class="err">
      <Alert tone="danger">{serverError}</Alert>
    </div>
  {/if}
</Card>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .lede {
    margin: 0 0 var(--sp-4);
    line-height: var(--lh-relaxed);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    padding: var(--sp-3) var(--sp-4);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    margin-bottom: var(--sp-3);
  }
  .status strong {
    display: block;
    color: var(--text-1);
    margin-bottom: 2px;
  }
  .status[data-tone='ok'] {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .status[data-tone='warn'] {
    background: var(--warn-soft, var(--surface-2));
    border-color: var(--warn, var(--border-subtle));
  }
  .badge {
    padding: 2px var(--sp-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
  }
  .badge.ok {
    background: var(--accent);
    color: white;
  }
  .form {
    display: grid;
    gap: var(--sp-3);
  }
  .zona-secao {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-3);
  }
  .form > :global(button[type='submit']) {
    justify-self: start;
  }
  .err {
    margin-top: var(--sp-3);
  }
</style>
