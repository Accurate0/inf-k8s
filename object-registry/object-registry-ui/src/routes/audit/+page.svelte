<script lang="ts">
	import { browser } from '$app/environment';
	import { invalidateAll, goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { listAuditLogs, type AuditLog } from '$lib/api';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Combobox } from '$lib/components/ui/combobox';
	import Badge from '$lib/components/Badge.svelte';
	import { toast } from 'svelte-sonner';
	import { RefreshCw, ArrowLeft, Filter, X } from '@lucide/svelte';
	import AuditTable from '$lib/components/AuditTable.svelte';

	let { data } = $props();

	let auditLogs: AuditLog[] = $state(data.auditLogs || []);
	let loading = $state(false);
	let error: string | null = $state(null);

	// Filter states
	let selectedActions = $state($page.url.searchParams.getAll('action'));
	let selectedSubjects = $state($page.url.searchParams.getAll('subject'));
	let selectedNamespaces = $state($page.url.searchParams.getAll('namespace'));

	let suggestions = $derived(data.suggestions || { actions: [], subjects: [], namespaces: [] });

	function addTag(category: 'action' | 'subject' | 'namespace', value: string) {
		const val = value.trim();
		if (!val) return;

		if (category === 'action' && !selectedActions.includes(val)) {
			selectedActions = [...selectedActions, val];
		} else if (category === 'subject' && !selectedSubjects.includes(val)) {
			selectedSubjects = [...selectedSubjects, val];
		} else if (category === 'namespace' && !selectedNamespaces.includes(val)) {
			selectedNamespaces = [...selectedNamespaces, val];
		}
	}

	function removeTag(category: 'action' | 'subject' | 'namespace', value: string) {
		if (category === 'action') {
			selectedActions = selectedActions.filter((v) => v !== value);
		} else if (category === 'subject') {
			selectedSubjects = selectedSubjects.filter((v) => v !== value);
		} else if (category === 'namespace') {
			selectedNamespaces = selectedNamespaces.filter((v) => v !== value);
		}
	}

	function handleFilter() {
		const url = new URL($page.url);
		url.searchParams.delete('action');
		url.searchParams.delete('subject');
		url.searchParams.delete('namespace');

		selectedActions.forEach(a => url.searchParams.append('action', a));
		selectedSubjects.forEach(s => url.searchParams.append('subject', s));
		selectedNamespaces.forEach(n => url.searchParams.append('namespace', n));

		goto(url.toString(), { keepFocus: true, noScroll: true });
	}

	function clearFilters() {
		selectedActions = [];
		selectedSubjects = [];
		selectedNamespaces = [];
		goto($page.url.pathname);
	}

	$effect(() => {
		auditLogs = data.auditLogs || [];
	});
</script>

<svelte:head>
	<title>Audit Logs - Object Registry</title>
</svelte:head>

<div class="container mx-auto max-w-7xl space-y-6 px-4 py-10 sm:px-6 lg:px-8">
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-4">
			<Button variant="ghost" size="icon" href="/" title="Back to Registry">
				<ArrowLeft class="h-4 w-4" />
			</Button>
			<h1 class="text-3xl font-bold tracking-tight">Audit History</h1>
		</div>
		<Button
			variant="outline"
			size="icon"
			onclick={async () => {
				await invalidateAll();
			}}
			disabled={loading}
		>
			<RefreshCw class="h-4 w-4 {loading ? 'animate-spin' : ''}" />
		</Button>
	</div>

	<div class="bg-muted/30 p-4 sm:p-5 rounded-lg border shadow-sm">
		<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 sm:gap-6 items-start mb-4">
			<div class="space-y-2">
				<Label for="actions" class="px-1">Actions</Label>
				<Combobox 
					options={suggestions.actions} 
					placeholder="Add action..." 
					onSelect={(v) => addTag('action', v)}
				/>
			</div>

			<div class="space-y-2">
				<Label for="subjects" class="px-1">Subjects</Label>
				<Combobox 
					options={suggestions.subjects} 
					placeholder="Add subject..." 
					onSelect={(v) => addTag('subject', v)}
				/>
			</div>

			<div class="space-y-2">
				<Label for="namespaces" class="px-1">Namespaces</Label>
				<Combobox 
					options={suggestions.namespaces} 
					placeholder="Add namespace..." 
					onSelect={(v) => addTag('namespace', v)}
				/>
			</div>

			<div class="space-y-2">
				<Label class="invisible hidden lg:block">Actions</Label>
				<div class="flex gap-2">
					<Button class="flex-1" onclick={handleFilter}>
						<Filter class="h-4 w-4 mr-2" />
						Apply Filters
					</Button>
					<Button variant="outline" onclick={clearFilters}>
						Clear
					</Button>
				</div>
			</div>
		</div>

		{#if selectedActions.length > 0 || selectedSubjects.length > 0 || selectedNamespaces.length > 0}
			<div class="flex flex-wrap gap-2 pt-2 border-t border-muted-foreground/10 mt-2">
				{#each selectedActions as action}
					<Badge variant="secondary" class="pl-2 pr-1 py-0.5 gap-1">
						<span class="text-[10px] uppercase font-bold opacity-50 mr-1">Action</span>
						{action}
						<button onclick={() => removeTag('action', action)} class="hover:bg-muted rounded-full p-0.5">
							<X class="h-3 w-3" />
						</button>
					</Badge>
				{/each}
				{#each selectedSubjects as subject}
					<Badge variant="secondary" class="pl-2 pr-1 py-0.5 gap-1">
						<span class="text-[10px] uppercase font-bold opacity-50 mr-1">Subject</span>
						{subject}
						<button onclick={() => removeTag('subject', subject)} class="hover:bg-muted rounded-full p-0.5">
							<X class="h-3 w-3" />
						</button>
					</Badge>
				{/each}
				{#each selectedNamespaces as ns}
					<Badge variant="secondary" class="pl-2 pr-1 py-0.5 gap-1">
						<span class="text-[10px] uppercase font-bold opacity-50 mr-1">Namespace</span>
						{ns}
						<button onclick={() => removeTag('namespace', ns)} class="hover:bg-muted rounded-full p-0.5">
							<X class="h-3 w-3" />
						</button>
					</Badge>
				{/each}
			</div>
		{/if}
	</div>

	<AuditTable
		{auditLogs}
		{loading}
		{error}
	/>
</div>

<style>
	:global(body) {
		background-color: hsl(var(--background));
		color: hsl(var(--foreground));
	}
</style>
