<script lang="ts">
  // Placar público dedicado — 1 mandato. Foco visual no dado (números
  // grandes), botão compartilhar (Web Share API + fallback copy link),
  // botão publicar-no-fediverso (cria uma nota com o link).
  import { onMount } from 'svelte';
  import {
    getMandate,
    getScorecard,
    postNote,
    type MandateDto,
    type ScorecardDto,
  } from '../../lib/api';
  import { responseRate, formatLatency } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Spinner from '../ui/Spinner.svelte';

  interface Props {
    mandateId: string;
  }
  let { mandateId }: Props = $props();

  let loading = $state(true);
  let mandate = $state<MandateDto | null>(null);
  let scorecard = $state<ScorecardDto | null>(null);
  let publishing = $state(false);
  let msg = $state<{ kind: 'ok' | 'error' | 'info'; text: string } | null>(null);

  onMount(async () => {
    const [m, s] = await Promise.all([getMandate(mandateId), getScorecard(mandateId)]);
    loading = false;
    if (m.ok && m.data) mandate = m.data;
    if (s.ok && s.data) scorecard = s.data;
  });

  const rate = $derived.by(() => {
    if (!scorecard) return null;
    return responseRate(scorecard.answered, scorecard.ignored);
  });

  const total = $derived((scorecard?.answered ?? 0) + (scorecard?.ignored ?? 0));
  const rateTone = $derived.by(() => {
    if (rate == null) return 'neutral';
    if (rate >= 70) return 'ok';
    if (rate >= 40) return 'warn';
    return 'bad';
  });

  function share() {
    if (typeof window === 'undefined') return;
    const url = window.location.href;
    const title = mandate
      ? `Placar de ${mandate.display_name} — DemocraciaBR`
      : 'Placar do parlamentar — DemocraciaBR';
    if (navigator.share) {
      navigator.share({ title, url }).catch(() => {});
    } else {
      navigator.clipboard?.writeText(url);
      msg = { kind: 'ok', text: 'Link copiado.' };
    }
  }

  async function publishToFediverse() {
    if (!mandate || publishing) return;
    publishing = true;
    msg = null;
    const url = window.location.href;
    const rate_bit = rate != null ? ` — ${rate}% de resposta` : '';
    const content =
      `📊 Placar público de accountability: ${mandate.display_name}${rate_bit}.\n\n` +
      `Propostas cidadãs respondidas × silêncio público registrado.\n\n${url}\n\n#DemocraciaBR`;
    const res = await postNote(content);
    publishing = false;
    msg = res.success
      ? { kind: 'ok', text: 'Publicado no fediverso.' }
      : {
          kind: 'error',
          text: res.error?.message ?? 'Não foi possível publicar.',
        };
  }
</script>

{#if loading}
  <div class="center"><Spinner /></div>
{:else if !mandate}
  <Card>
    <Alert tone="danger">Mandato não encontrado.</Alert>
  </Card>
{:else}
  <Card>
    <header class="head">
      {#if mandate.avatar_url}
        <img class="avatar" src={mandate.avatar_url} alt="" />
      {:else}
        <span class="avatar avatar-placeholder">👤</span>
      {/if}
      <div class="who">
        <p class="kicker">Placar público</p>
        <h1>{mandate.display_name}</h1>
        <p class="meta muted">
          {#if mandate.party}{mandate.party}/{mandate.uf} · {/if}{mandate.office}
        </p>
      </div>
    </header>

    {#if scorecard && total > 0}
      <div class="hero" data-tone={rateTone}>
        {#if rate != null}
          <p class="rate">
            <strong>{rate}%</strong> <span>respondidas dentro do prazo</span>
          </p>
        {/if}
        <p class="denominator muted">
          {total} demanda{total === 1 ? '' : 's'} cidadã{total === 1 ? '' : 's'} registrada{total === 1 ? '' : 's'} até agora
        </p>
      </div>

      <div class="grid">
        <div class="stat ok">
          <strong>{scorecard.answered}</strong>
          <span>respondidas dentro do prazo</span>
        </div>
        <div class="stat bad">
          <strong>{scorecard.ignored}</strong>
          <span>silêncio público registrado</span>
        </div>
        {#if scorecard.median_response_hours != null}
          <div class="stat">
            <strong>{formatLatency(scorecard.median_response_hours)}</strong>
            <span>tempo médio de resposta</span>
          </div>
        {/if}
      </div>
    {:else}
      <div class="empty">
        <p>
          <strong>Nenhuma demanda cidadã registrada até agora.</strong>
        </p>
        <p class="muted">
          Assim que uma proposta com apoio suficiente for enviada, o relógio
          começa a correr — resposta dentro do prazo entra na coluna verde;
          silêncio, na vermelha.
        </p>
        <Button href={`/propor?mandate=${mandate.id}`} variant="primary">
          Propor demanda a {mandate.display_name.split(' ')[0]}
        </Button>
      </div>
    {/if}

    <footer class="actions">
      <Button onclick={share} variant="ghost">Compartilhar</Button>
      <Button
        onclick={publishToFediverse}
        loading={publishing}
        variant="primary"
      >
        Publicar no fediverso
      </Button>
      <a class="alt muted" href={`/politicos/?id=${mandate.id}`}>Ver perfil completo →</a>
    </footer>
    {#if msg}
      <p class={`msg ${msg.kind}`} role="status">{msg.text}</p>
    {/if}
  </Card>
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: var(--sp-6);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    object-fit: cover;
  }
  .avatar-placeholder {
    background: var(--surface-2);
    display: grid;
    place-items: center;
    font-size: 32px;
  }
  .who {
    flex: 1;
  }
  .kicker {
    margin: 0;
    font-size: var(--fs-xs);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-3);
    font-weight: var(--fw-bold);
  }
  h1 {
    margin: 4px 0 2px;
    font-size: var(--fs-2xl);
  }
  .meta {
    margin: 0;
  }

  .hero {
    padding: var(--sp-4);
    border-radius: var(--r-base);
    background: var(--surface-2);
    text-align: center;
    margin-bottom: var(--sp-4);
    border: 1px solid var(--border-subtle);
  }
  .hero[data-tone='ok'] {
    background: var(--c-green-soft, #e6f7ed);
    border-color: #b7e4c7;
  }
  .hero[data-tone='warn'] {
    background: #fff7e6;
    border-color: #ffd591;
  }
  .hero[data-tone='bad'] {
    background: #fff1f0;
    border-color: #ffa39e;
  }
  .rate {
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .rate strong {
    display: block;
    font-size: 3rem;
    line-height: 1.05;
    font-variant-numeric: tabular-nums;
  }
  .denominator {
    margin: 8px 0 0;
    font-size: var(--fs-sm);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .stat {
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    text-align: center;
  }
  .stat strong {
    display: block;
    font-size: var(--fs-xl);
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }
  .stat span {
    display: block;
    font-size: var(--fs-sm);
    color: var(--text-2);
    margin-top: 4px;
  }
  .stat.ok strong {
    color: #115c2d;
  }
  .stat.bad strong {
    color: #a8071a;
  }

  .empty {
    display: grid;
    gap: var(--sp-3);
    padding: var(--sp-5);
    text-align: center;
  }

  .actions {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
    flex-wrap: wrap;
    justify-content: center;
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border-subtle);
  }
  .alt {
    margin-left: auto;
    font-size: var(--fs-sm);
    text-decoration: none;
  }
  .msg {
    margin: var(--sp-3) 0 0;
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    text-align: center;
  }
  .msg.ok {
    background: var(--c-green-soft, #e6f7ed);
    color: #115c2d;
  }
  .msg.error {
    background: #fff1f0;
    color: #a8071a;
  }
  .msg.info {
    background: var(--surface-2);
  }
</style>
