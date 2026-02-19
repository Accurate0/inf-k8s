<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { Search } from "@lucide/svelte";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: CommandPrimitive.InputProps = $props();
</script>

<div class="flex items-center border-b px-3" data-slot="command-input-wrapper">
	<Search class="mr-2 h-4 w-4 shrink-0 opacity-50" />
	<CommandPrimitive.Input
		bind:ref
		class={cn(
			"placeholder:text-muted-foreground flex h-11 w-full rounded-md bg-transparent py-3 text-sm outline-none disabled:cursor-not-allowed disabled:opacity-50",
			className
		)}
		{...restProps}
	/>
</div>
