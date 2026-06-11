<script lang="ts">
  import * as Select from "$lib/components/ui/select";

  type Option = { value: string; label: string };

  let {
    value = $bindable(),
    options,
    placeholder = "Select…",
    name,
    class: className,
  }: {
    value: string;
    options: Option[];
    placeholder?: string;
    name?: string;
    class?: string;
  } = $props();

  const selectedLabel = $derived(options.find((o) => o.value === value)?.label ?? placeholder);
</script>

{#if name}
  <input type="hidden" {name} {value} />
{/if}
<Select.Root type="single" bind:value>
  <Select.Trigger class={className}>{selectedLabel}</Select.Trigger>
  <Select.Content>
    {#each options as option (option.value)}
      <Select.Item value={option.value} label={option.label} />
    {/each}
  </Select.Content>
</Select.Root>
