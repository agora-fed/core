<script lang="ts">
  // Small reusable upload widget — file picker + immediate POST + status. The parent passes the
  // current URL (for preview) and a callback that runs with the refreshed profile after upload.
  import { uploadProfileImage, type ProfileDto } from '../../lib/api';

  let {
    kind,
    currentUrl,
    label,
    helper,
    aspect = '1 / 1',
    onUploaded,
  }: {
    kind: 'avatar' | 'cover';
    currentUrl: string | null;
    label: string;
    helper: string;
    aspect?: string;
    onUploaded: (profile: ProfileDto) => void;
  } = $props();

  let inputEl = $state<HTMLInputElement | null>(null);
  let busy = $state(false);
  let status = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  // Local preview so the user sees their pick instantly (the persisted URL only refreshes after
  // the upload completes; this preview is replaced once the server responds).
  let preview = $state<string | null>(null);

  function pick() {
    inputEl?.click();
  }

  async function onChange(event: Event) {
    const target = event.target as HTMLInputElement;
    const file = target.files?.[0];
    if (!file) return;

    if (file.size > 5 * 1024 * 1024) {
      status = { kind: 'error', text: 'Imagem maior que 5 MB. Escolha outra.' };
      target.value = '';
      return;
    }
    if (!/^image\/(png|jpe?g|webp)$/i.test(file.type)) {
      status = {
        kind: 'error',
        text: 'Use uma imagem PNG, JPEG ou WebP.',
      };
      target.value = '';
      return;
    }

    // Local preview while we wait for the server.
    preview = URL.createObjectURL(file);
    status = null;
    busy = true;

    const res = await uploadProfileImage(kind, file);
    busy = false;
    target.value = '';

    if (res.success && res.data) {
      onUploaded(res.data);
      status = { kind: 'ok', text: 'Imagem atualizada.' };
      // Clear the local preview — the next render uses the persisted URL.
      if (preview) {
        URL.revokeObjectURL(preview);
        preview = null;
      }
    } else {
      // Roll back the preview on failure.
      if (preview) {
        URL.revokeObjectURL(preview);
        preview = null;
      }
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar a imagem.',
      };
    }
  }
</script>

<div class="upload">
  <span class="label">{label}</span>
  <div
    class="frame"
    class:square={kind === 'avatar'}
    style="aspect-ratio: {aspect};"
  >
    {#if preview ?? currentUrl}
      <img src={preview ?? currentUrl} alt="" />
    {:else}
      <span class="placeholder" aria-hidden="true">
        {kind === 'avatar' ? '👤' : '🖼️'}
      </span>
    {/if}
    {#if busy}
      <span class="overlay" role="status">Enviando…</span>
    {/if}
  </div>
  <input
    bind:this={inputEl}
    type="file"
    accept="image/png,image/jpeg,image/webp"
    onchange={onChange}
    hidden
  />
  <div class="actions">
    <button type="button" class="btn btn-ghost" onclick={pick} disabled={busy}>
      {currentUrl ? 'Trocar' : 'Enviar'} imagem
    </button>
    <p class="hint muted">{helper}</p>
  </div>
  {#if status}
    <p class={`hint ${status.kind === 'error' ? 'hint-error' : 'hint-ok'}`} role="status">
      {status.text}
    </p>
  {/if}
</div>

<style>
  .upload {
    display: grid;
    gap: 0.6rem;
  }
  .label {
    font-weight: 600;
    color: var(--c-text);
    font-size: 0.95rem;
  }
  .frame {
    position: relative;
    background: var(--c-bg);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    max-width: 14rem;
  }
  .frame.square {
    max-width: 9rem;
    border-radius: 50%;
  }
  .frame img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .placeholder {
    font-size: 2.2rem;
    color: var(--c-text-muted);
  }
  .overlay {
    position: absolute;
    inset: 0;
    background: rgba(255, 255, 255, 0.75);
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 600;
    color: var(--c-navy);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .actions .hint {
    margin: 0;
    flex: 1;
    min-width: 12rem;
  }
  .hint-ok {
    color: var(--c-green-dark);
  }
</style>
