<script lang="ts">
  import { valueTypeOptions } from "$lib/labels";
  import { ValueType } from "@accurate0/feature-flag-client/model";
  import * as Card from "$lib/components/ui/card";
  import { Button, buttonVariants } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import FieldSelect from "$lib/components/field-select.svelte";

  let { form } = $props();

  let valueType = $state<string>(String(ValueType.VALUE_TYPE_BOOLEAN));
  const vt = $derived(Number(valueType));
  let enabled = $state(false);
  let variants = $state<{ key: string; value: string }[]>([
    { key: "on", value: "true" },
    { key: "off", value: "false" },
  ]);
  let defaultVariantKey = $state("on");

  let lastType = $state<string>(String(ValueType.VALUE_TYPE_BOOLEAN));
  $effect(() => {
    if (valueType === lastType) return;
    lastType = valueType;
    if (vt === ValueType.VALUE_TYPE_BOOLEAN) {
      variants = [
        { key: "on", value: "true" },
        { key: "off", value: "false" },
      ];
      defaultVariantKey = "on";
    } else {
      variants = [{ key: "", value: "" }];
      defaultVariantKey = "";
    }
  });

  function encode(value: string): unknown {
    switch (vt) {
      case ValueType.VALUE_TYPE_INTEGER:
      case ValueType.VALUE_TYPE_FLOAT:
        return Number(value);
      case ValueType.VALUE_TYPE_BOOLEAN:
      case ValueType.VALUE_TYPE_OBJECT:
        return JSON.parse(value);
      default:
        return value;
    }
  }

  const variantsJson = $derived.by(() => {
    try {
      return JSON.stringify(variants.filter((v) => v.key).map((v) => ({ key: v.key, value: encode(v.value) })));
    } catch {
      return "";
    }
  });

  const typeOptions = valueTypeOptions.map((o) => ({ value: String(o.value), label: o.label }));
  const variantKeyOptions = $derived(variants.filter((v) => v.key).map((v) => ({ value: v.key, label: v.key })));
</script>

<a class="mb-4 inline-block text-sm text-muted-foreground hover:underline" href="/">&larr; Flags</a>

<Card.Root>
  <Card.Header>
    <Card.Title>New flag</Card.Title>
  </Card.Header>
  <Card.Content>
    {#if form?.message}
      <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {form.message}
      </p>
    {/if}

    <form method="POST" class="space-y-5">
      <input type="hidden" name="valueType" value={valueType} />
      <input type="hidden" name="variants" value={variantsJson} />
      <input type="hidden" name="defaultVariantKey" value={defaultVariantKey} />

      <div class="grid gap-2">
        <Label for="key">Key</Label>
        <Input id="key" name="key" required value={form?.values?.key ?? ""} />
      </div>

      <div class="grid gap-2">
        <Label>Value type</Label>
        <FieldSelect bind:value={valueType} options={typeOptions} class="w-48" />
      </div>

      <fieldset class="rounded-md border p-4">
        <legend class="px-1 text-sm font-medium">Variants</legend>
        <div class="space-y-2">
          {#each variants as variant, i (i)}
            <div class="flex gap-2">
              <Input placeholder="key" bind:value={variant.key} class="flex-1" />
              {#if vt === ValueType.VALUE_TYPE_BOOLEAN}
                <FieldSelect
                  bind:value={variant.value}
                  options={[
                    { value: "true", label: "true" },
                    { value: "false", label: "false" },
                  ]}
                  class="w-32"
                />
              {:else if vt === ValueType.VALUE_TYPE_INTEGER || vt === ValueType.VALUE_TYPE_FLOAT}
                <Input type="number" placeholder="number" bind:value={variant.value} class="flex-1" />
              {:else if vt === ValueType.VALUE_TYPE_OBJECT}
                <Input placeholder="JSON object" bind:value={variant.value} class="flex-1" />
              {:else}
                <Input placeholder="string value" bind:value={variant.value} class="flex-1" />
              {/if}
              <Button type="button" variant="ghost" size="icon" onclick={() => variants.splice(i, 1)}>×</Button>
            </div>
          {/each}
        </div>
        <Button type="button" variant="outline" size="sm" class="mt-3" onclick={() => variants.push({ key: "", value: "" })}>
          Add variant
        </Button>
      </fieldset>

      <div class="grid gap-2">
        <Label>Default variant</Label>
        <FieldSelect bind:value={defaultVariantKey} options={variantKeyOptions} class="w-48" />
      </div>

      <label class="flex items-center gap-2 text-sm">
        <Checkbox bind:checked={enabled} /> Enabled
        <input type="hidden" name="enabled" value={enabled ? "on" : "off"} />
      </label>

      <Button type="submit">Create</Button>
    </form>
  </Card.Content>
</Card.Root>
