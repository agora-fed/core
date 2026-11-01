<script lang="ts">
  import { onMount } from 'svelte';
  import {
    attendeeLabel,
    attendMeeting,
    formatMeetingDate,
    getMeeting,
    getMinutes,
    listAttendees,
    listMeetings,
    localToIso,
    scheduleMeeting,
    setMinutes,
    splitMeetings,
    type MeetingDto,
  } from '../../lib/meetings';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let logged = $state(false);
  let meetings = $state<MeetingDto[]>([]);
  let current = $state<MeetingDto | null>(null);
  let attendees = $state(0);
  let busy = $state(false);

  let title = $state('');
  let location = $state('');
  let startsAt = $state('');
  let minutesDraft = $state('');
  // Stays false unless the API lets this person read the minutes, which only org admins can.
  let canSeeMinutes = $state(false);
  let minutes = $state<string | null>(null);

  const groups = $derived(splitMeetings(meetings, new Date()));

  async function loadList() {
    const res = await listMeetings();
    if (res.ok && res.data) meetings = res.data;
    else loadError = res.error ?? 'Não foi possível carregar as reuniões.';
  }

  async function loadDetail(id: string) {
    const [meeting, people] = await Promise.all([getMeeting(id), listAttendees(id)]);
    if (meeting.ok && meeting.data) {
      current = meeting.data;
      attendees = people.data?.length ?? 0;
      if (logged) {
        const record = await getMinutes(id);
        canSeeMinutes = record.ok;
        minutes = record.data?.minutes ?? null;
        if (!minutesDraft) minutesDraft = minutes ?? '';
      }
    } else {
      loadError = meeting.error ?? 'Reunião não encontrada.';
    }
  }

  onMount(async () => {
    try {
      logged = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    const id = new URLSearchParams(window.location.search).get('id');
    await (id ? loadDetail(id) : loadList());
    loading = false;
  });

  async function schedule(event: SubmitEvent) {
    event.preventDefault();
    const starts_at = localToIso(startsAt);
    if (!starts_at) {
      toast.error('Informe a data e a hora da reunião.');
      return;
    }
    busy = true;
    const res = await scheduleMeeting({ title, location, starts_at });
    busy = false;
    if (res.success && res.data) {
      window.location.href = `/reunioes/?id=${res.data.id}`;
    } else {
      toast.error(res.error?.message ?? 'Não foi possível agendar a reunião.');
    }
  }

  async function attend() {
    if (!current) return;
    busy = true;
    const res = await attendMeeting(current.id);
    busy = false;
    if (res.success) {
      toast.success('Presença confirmada.');
      await loadDetail(current.id);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível confirmar a presença.');
    }
  }

  async function saveMinutes(event: SubmitEvent) {
    event.preventDefault();
    if (!current) return;
    busy = true;
    const res = await setMinutes(current.id, minutesDraft);
    busy = false;
    if (res.success) {
      toast.success('Ata salva.');
      await loadDetail(current.id);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível salvar a ata.');
    }
  }
</script>

{#snippet meetingList(items: MeetingDto[], empty: string)}
  {#if items.length === 0}
    <p class="muted">{empty}</p>
  {:else}
    <ul class="list">
      {#each items as m (m.id)}
        <li>
          <a href={`/reunioes/?id=${m.id}`}>{m.title}</a>
          <span class="muted">{formatMeetingDate(m.starts_at)} · {m.location}</span>
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <Card><p role="alert">{loadError}</p></Card>
{:else if current}
  <p><a href="/reunioes/">← Todas as reuniões</a></p>
  <h1>{current.title}</h1>
  <p class="muted">{formatMeetingDate(current.starts_at)} · {current.location}</p>
  <p>{attendeeLabel(attendees)}</p>
  {#if logged}
    <Button disabled={busy} onclick={attend}>Confirmar presença</Button>
  {:else}
    <p class="muted"><a href="/entrar">Entre</a> para confirmar presença.</p>
  {/if}

  {#if canSeeMinutes}
    <h2>Ata</h2>
    {#if minutes}
      <p class="minutes">{minutes}</p>
    {:else}
      <p class="muted">A ata ainda não foi registrada.</p>
    {/if}
    <form onsubmit={saveMinutes}>
      <label>
        Registrar ata
        <textarea bind:value={minutesDraft} rows="8" maxlength="50000" required></textarea>
      </label>
      <Button type="submit" variant="ghost" disabled={busy}>Salvar ata</Button>
    </form>
  {/if}
{:else}
  <h1>Reuniões</h1>
  <h2>Próximas</h2>
  {@render meetingList(groups.upcoming, 'Nenhuma reunião agendada.')}
  <h2>Anteriores</h2>
  {@render meetingList(groups.past, 'Nenhuma reunião anterior.')}

  {#if logged}
    <Card>
      <h2>Agendar reunião</h2>
      <form onsubmit={schedule}>
        <label>Assunto <input bind:value={title} maxlength="200" required /></label>
        <label>Local ou link <input bind:value={location} maxlength="500" required /></label>
        <label>Data e hora <input type="datetime-local" bind:value={startsAt} required /></label>
        <Button type="submit" disabled={busy}>Agendar</Button>
      </form>
    </Card>
  {/if}
{/if}

<style>
  .list {
    list-style: none;
    margin: 0 0 var(--sp-4);
    padding: 0;
  }
  .list li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  form {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-3);
    margin-top: var(--sp-3);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    width: 100%;
  }
  .minutes {
    white-space: pre-wrap;
  }
</style>
