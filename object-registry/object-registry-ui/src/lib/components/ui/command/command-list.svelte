<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: CommandPrimitive.ListProps = $props();
</script>

<CommandPrimitive.List
	bind:ref
	class={cn("max-h-[300px] overflow-y-auto overflow-x-hidden", className)}
	{...restProps}
>
	{@render children?.()}
</CommandPrimitive.List>
