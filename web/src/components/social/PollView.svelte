<script lang="ts">
  // Poll renderer + voter. Shows one bar per option with percentage + count.
  // Ballot form on top when the viewer hasn't voted and the poll is open;
  // switches to read-only "results" mode after voting or on expiry.
  import type { PollDto } from '../../lib/types';
  import { votePoll } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';

  interface Props {
    /** ActivityPub object URI of the Note this poll belongs to. */
    noteUri: string;
    poll: PollDto;
    loggedIn: boolean;
    /** Fired with the refreshed DTO on a successful vote so the parent card
     *  can swap its cached state without a full reload. Also updated
     *  internally so the bars re-render immediately without a round-trip. */
    onvoted?: (updated: PollDto) => void;
  }


  let { noteUri, poll: initialPoll, loggedIn, onvoted }: Props = $props();
  let poll = $state<PollDto>(initialPoll);
  $effect(() => {
    poll = initialPoll;
  });

  let selected = $state<Set<string>>(new Set());
  let busy = $state(false);
  let now = $state(Date.now());

  // Tick every 15s so the countdown updates without a page refresh.
  $effect(() => {
    const iv = setInterval(() => {
      now = Date.now();
    }, 15_000);
    return () => clearInterval(iv);
  });

  const expiresMs = $derived(new Date(poll.expires_at).getTime());
  const secondsLeft = $derived(Math.max(0, Math.floor((expiresMs - now) / 1000)));
  const isClosed = $derived(
    Boolean(poll.closed_at) || secondsLeft === 0,
  );
  const hasVoted = $derived(poll.voted_option_ids.length > 0);
  const showResults = $derived(hasVoted || isClosed);
  const total = $derived(Math.max(1, poll.total_votes));

  function toggle(id: string) {
    if (!loggedIn || showResults) return;
    const next = new Set(selected);
    if (poll.multiple) {
      if (next.has(id)) next.delete(id);
      else next.add(id);
    } else {
      next.clear();
      next.add(id);
    }
    selected = next;
  }

  async function submit() {
    if (busy || selected.size === 0) return;
    busy = true;
    const res = await votePoll(noteUri, Array.from(selected));
    busy = false;
    if (res.success && res.data) {
      poll = res.data;
      selected.clear();
      onvoted?.(res.data);
      toast.success('Voto registrado.');
    } else {
      toast.error(res.error?.message ?? 'Não foi possível votar.');
    }
  }

  function fmtLeft(s: number): string {
    if (s <= 0) return 'encerrada';
    if (s < 60) return `${s}s restantes`;
    if (s < 3600) return `${Math.floor(s / 60)}min restantes`;
    if (s < 86400) return `${Math.floor(s / 3600)}h restantes`;
    return `${Math.floor(s / 86400)}d restantes`;
  }
</script>

<div class="poll">
  <ol>
    {#each poll.options as opt (opt.id)}
      {@const pct = Math.round((opt.vote_count / total) * 100)}
      {@const chosen = poll.voted_option_ids.includes(opt.id)}
      {@const picked = selected.has(opt.id)}
      <li>
        {#if showResults}
          <div
            class="bar"
            class:winner={chosen}
            style={`--pct:${pct}%`}
            role="group"
            aria-label={`${opt.text}: ${pct}%`}
          >
            {#if chosen}
              <Icon name="check" size={12} />
            {/if}
            <span class="txt">{opt.text}</span>
            <span class="pct">{pct}%</span>
          </div>
        {:else}
          <label class="pick" class:picked>
            <input
              type={poll.multiple ? 'checkbox' : 'radio'}
              name={`poll-${poll.id}`}
              checked={picked}
              disabled={!loggedIn}
              onchange={() => toggle(opt.id)}
            />
            <span>{opt.text}</span>
          </label>
        {/if}
      </li>
    {/each}
  </ol>

  <div class="foot">
    <span class="meta">
      {poll.total_votes} {poll.total_votes === 1 ? 'voto' : 'votos'}
      · {fmtLeft(secondsLeft)}
      {#if poll.multiple}· múltipla escolha{/if}
    </span>
    {#if !showResults}
      <Button
        variant="primary"
        size="sm"
        disabled={!loggedIn || selected.size === 0 || busy}
        loading={busy}
        onclick={submit}
      >
        Votar
      </Button>
    {/if}
  </div>
  <p class="gate-note muted" title="A enquete circula no fediverso, mas o voto é local.">
    Apenas cidadãos com conta em <strong>democracia.social.br</strong> votam
    aqui. Fediversos parceiros veem a enquete, mas o voto de fora não conta.
  </p>
</div>

<style>
  .poll {
    margin: var(--sp-3) 0 var(--sp-1);
    padding: var(--sp-3);
    background: var(--surface-2);
    border-radius: var(--r-base);
    border: 1px solid var(--border-subtle);
  }
  ol {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .bar {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    overflow: hidden;
    font-size: var(--fs-sm);
    color: var(--text-1);
  }
  .bar::before {
    content: '';
    position: absolute;
    inset: 0 auto 0 0;
    width: var(--pct, 0%);
    background: color-mix(in srgb, var(--accent) 24%, transparent);
    transition: width var(--dur-base) var(--ease-out);
  }
  .bar.winner::before {
    background: color-mix(in srgb, var(--accent) 42%, transparent);
  }
  .bar > * {
    position: relative;
    z-index: 1;
  }
  .bar .txt {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar .pct {
    font-variant-numeric: tabular-nums;
    font-weight: var(--fw-semibold);
    color: var(--text-2);
  }

  .pick {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    cursor: pointer;
    font-size: var(--fs-sm);
    color: var(--text-1);
    transition: background var(--dur-fast) var(--ease-out), border-color var(--dur-fast) var(--ease-out);
  }
  .pick:hover {
    border-color: var(--border-strong);
  }
  .pick.picked {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .pick input {
    accent-color: var(--accent);
  }

  .gate-note {
    margin: var(--sp-2) 0 0;
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    line-height: var(--lh-snug);
  }
  .gate-note strong {
    color: var(--text-1);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    margin-top: var(--sp-3);
  }
  .meta {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
</style>
