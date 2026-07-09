<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListEmojis,
    adminUploadEmoji,
    adminToggleEmoji,
    adminDeleteEmoji,
    type CustomEmojiDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<CustomEmojiDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let file = $state<File | null>(null);
  let shortcode = $state('');
  let uploading = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListEmojis();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar.';
    }
  }
  onMount(reload);

  function onFilePicked(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    file = input.files?.[0] ?? null;
  }

  async function onUpload(e: SubmitEvent) {
    e.preventDefault();
    if (!file || !shortcode.trim()) return;
    uploading = true;
    const res = await adminUploadEmoji(file, shortcode.trim());
    uploading = false;
    if (res.success && res.data) {
      items = [res.data, ...items];
      shortcode = '';
      file = null;
      // Reset input
      const input = document.getElementById('emoji-file') as HTMLInputElement | null;
      if (input) input.value = '';
      toast.success('Emoji criado.');
    } else {
      toast.error(res.error?.message ?? 'Falha no upload.');
    }
  }

  async function onToggle(e: CustomEmojiDto) {
    if (busy.has(e.id)) return;
    busy = new Set(busy).add(e.id);
    const res = await adminToggleEmoji(e.id, !e.enabled);
    const done = new Set(busy);
    done.delete(e.id);
    busy = done;
    if (res.success) {
      items = items.map((it) => it.id === e.id ? { ...it, enabled: !it.enabled } : it);
    } else {
      toast.error(res.error?.message ?? 'Falha ao alternar.');
    }
  }

  async function onDelete(e: CustomEmojiDto) {
    if (busy.has(e.id) || !confirm(`Apagar :${e.shortcode}:?`)) return;
    busy = new Set(busy).add(e.id);
    const res = await adminDeleteEmoji(e.id);
    const done = new Set(busy);
    done.delete(e.id);
    busy = done;
    if (res.success) {
      items = items.filter((it) => it.id !== e.id);
      toast.success('Apagado.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao apagar.');
    }
  }
</script>

<Card>
  <h2 class="sub">Adicionar emoji</h2>
  <form onsubmit={onUpload} class="form">
    <label class="fld">
      <span>Arquivo (PNG/JPG/WebP/GIF até 512 KB)</span>
      <input id="emoji-file" type="file" accept="image/png,image/jpeg,image/webp,image/gif" onchange={onFilePicked} />
    </label>
    <label class="fld">
      <span>Shortcode (sem <code>:</code>)</span>
      <input
        type="text"
        bind:value={shortcode}
        placeholder="ex.: party_dbr"
        pattern="[A-Za-z0-9_-]{2,32}"
      />
    </label>
    <Button type="submit" variant="primary" loading={uploading} disabled={!file || !shortcode.trim()}>
      Enviar
    </Button>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card><EmptyState icon="upload" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="upload"
        title="Sem emojis ainda"
        description="Adicione o primeiro acima. Depois use como :shortcode: no compose."
      />
    </Card>
  {:else}
    <ul class="grid">
      {#each items as e (e.id)}
        <li>
          <Card>
            <div class="e">
              <img class="img" src={e.url} alt={`:${e.shortcode}:`} />
              <div class="meta">
                <code>:{e.shortcode}:</code>
                {#if !e.enabled}<Badge tone="neutral" size="sm">desativado</Badge>{/if}
              </div>
              <div class="a">
                <Button variant="ghost" size="sm" onclick={() => onToggle(e)} loading={busy.has(e.id)}>
                  {e.enabled ? 'Desativar' : 'Ativar'}
                </Button>
                <Button variant="danger" size="sm" onclick={() => onDelete(e)} loading={busy.has(e.id)}>
                  Apagar
                </Button>
              </div>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sub { margin: 0 0 var(--sp-3); font-size: var(--fs-md); }
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
  }
  .fld input {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  @media (max-width: 800px) {
    .form { grid-template-columns: 1fr; }
  }
  .list { margin-top: var(--sp-4); }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  }
  .e {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-2);
  }
  .img {
    width: 64px;
    height: 64px;
    object-fit: contain;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    padding: 4px;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
  }
  .meta code {
    font-family: ui-monospace, SFMono-Regular, monospace;
  }
  .a {
    display: flex;
    gap: var(--sp-1);
    flex-wrap: wrap;
    justify-content: center;
  }
</style>
