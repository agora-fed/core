<script lang="ts">
  // The SLA countdown clock — the emotional core of the platform.
  // A live, ticking public deadline an official cannot silently ignore.
  import type { SlaStatus } from '../../lib/types';

  interface Props {
    dueAt: string;
    startedAt?: string;
    status: SlaStatus;
    /** Public display name of the official under the clock. */
    mandateName?: string;
  }

  let { dueAt, startedAt, status, mandateName }: Props = $props();

  const due = new Date(dueAt).getTime();
  const started = startedAt ? new Date(startedAt).getTime() : due;

  let now = $state(Date.now());

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  // Remaining milliseconds (clamped at zero).
  let remaining = $derived(Math.max(0, due - now));
  let expired = $derived(remaining === 0);
  let settled = $derived(
    status === 'answered' || status === 'acted' || status === 'ignored',
  );

  function part(value: number): string {
    return value.toString().padStart(2, '0');
  }

  let days = $derived(Math.floor(remaining / 86_400_000));
  let hours = $derived(Math.floor((remaining % 86_400_000) / 3_600_000));
  let minutes = $derived(Math.floor((remaining % 3_600_000) / 60_000));
  let seconds = $derived(Math.floor((remaining % 60_000) / 1000));

  // Fraction of the SLA window already consumed (0..1) for the progress ring.
  let elapsedFrac = $derived(
    due > started ? Math.min(1, (now - started) / (due - started)) : 1,
  );

  // Urgency tier drives the color of the live clock.
  let tier = $derived(
    settled
      ? status
      : expired
        ? 'ignored'
        : remaining < 86_400_000
          ? 'urgent'
          : 'pending',
  );

  const LABEL: Record<string, string> = {
    pending: 'Prazo correndo',
    urgent: 'Prazo se esgotando',
    answered: 'Respondido no prazo',
    acted: 'Respondido com compromisso',
    ignored: 'Ignorado — silêncio público',
  };
</script>

<div
  class={`clock tier-${tier}`}
  role="group"
  aria-label="Relógio do prazo de resposta"
>
  <div class="head">
    <span class="status-dot" aria-hidden="true"></span>
    <span class="status-label">{LABEL[tier] ?? 'Prazo'}</span>
  </div>

  {#if settled}
    <p class="settled-msg">
      {#if status === 'ignored'}
        O prazo terminou sem resposta. O silêncio está registrado em público.
      {:else}
        {mandateName ?? 'O mandato'} respondeu dentro do prazo.
      {/if}
    </p>
  {:else}
    <div class="digits">
      {#if days > 0}
        <span class="seg"><strong>{days}</strong><small>dias</small></span>
        <span class="colon">:</span>
      {/if}
      <span class="seg"><strong>{part(hours)}</strong><small>h</small></span>
      <span class="colon">:</span>
      <span class="seg"><strong>{part(minutes)}</strong><small>min</small></span>
      <span class="colon">:</span>
      <span class="seg"><strong>{part(seconds)}</strong><small>s</small></span>
    </div>
    <div
      class="bar"
      role="progressbar"
      aria-valuemin="0"
      aria-valuemax="100"
      aria-valuenow={Math.round(elapsedFrac * 100)}
      aria-label="Tempo decorrido do prazo"
    >
      <span class="fill" style={`width:${elapsedFrac * 100}%`}></span>
    </div>
    <p class="caption">
      {expired
        ? 'Prazo esgotado, aguardando registro.'
        : 'Tempo restante para o mandato responder publicamente.'}
    </p>
  {/if}
</div>

<style>
  .clock {
    --accent: var(--c-pending);
    border: 1px solid var(--c-border);
    border-radius: 16px;
    padding: 1.1rem 1.25rem 1.25rem;
    background: #fff;
    box-shadow: var(--shadow);
  }
  .tier-pending {
    --accent: #b45309;
  }
  .tier-urgent {
    --accent: #c2410c;
    background: #fff7f3;
  }
  .tier-answered,
  .tier-acted {
    --accent: var(--c-green);
    background: var(--c-green-soft);
  }
  .tier-ignored {
    --accent: #b91c1c;
    background: #fef2f2;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.6rem;
  }
  .status-dot {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 0 var(--accent);
  }
  .tier-pending .status-dot,
  .tier-urgent .status-dot {
    animation: pulse 1.6s ease-out infinite;
  }
  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 60%, transparent);
    }
    100% {
      box-shadow: 0 0 0 10px transparent;
    }
  }
  .status-label {
    font-weight: 700;
    color: var(--accent);
    font-size: 0.95rem;
    letter-spacing: 0.01em;
  }
  .digits {
    display: flex;
    align-items: baseline;
    gap: 0.3rem;
    font-variant-numeric: tabular-nums;
    color: var(--c-navy);
  }
  .seg {
    display: inline-flex;
    flex-direction: column;
    align-items: center;
    min-width: 2.6ch;
  }
  .seg strong {
    font-size: clamp(1.8rem, 7vw, 2.6rem);
    font-weight: 700;
    line-height: 1;
  }
  .seg small {
    font-size: 0.7rem;
    color: var(--c-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .colon {
    font-size: 1.6rem;
    color: var(--c-text-muted);
    align-self: flex-start;
    margin-top: 0.1rem;
  }
  .bar {
    height: 7px;
    border-radius: 999px;
    background: #e7e2da;
    overflow: hidden;
    margin: 0.85rem 0 0.5rem;
  }
  .tier-ignored .bar {
    background: #f3d4d4;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 1s linear;
  }
  .caption {
    margin: 0;
    font-size: 0.85rem;
    color: var(--c-text-muted);
  }
  .settled-msg {
    margin: 0;
    font-weight: 500;
    color: var(--c-navy);
  }
</style>
