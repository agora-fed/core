<script lang="ts">
  // Icon: resolves an id in /icons.svg. Sized via CSS `width/height`;
  // color inherits via `currentColor`. Set `label` for a11y (or leave empty
  // to mark as decorative — hidden from screen readers).
  interface Props {
    name: string;
    size?: number | string;
    label?: string;
    class?: string;
  }
  let { name, size = 20, label, class: klass = '' }: Props = $props();
  const dim = typeof size === 'number' ? `${size}px` : size;
</script>

<svg
  class={`icon ${klass}`}
  style={`width:${dim};height:${dim}`}
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : true}
  focusable="false"
>
  <use href={`/icons.svg#i-${name}`} />
</svg>

<style>
  .icon {
    display: inline-block;
    vertical-align: middle;
    flex-shrink: 0;
    color: inherit;
  }
</style>
