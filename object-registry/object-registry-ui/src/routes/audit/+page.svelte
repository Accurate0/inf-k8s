<script lang="ts">
	import { browser } from '$app/environment';
	import { invalidateAll } from '$app/navigation';
	import { listAuditLogs, type AuditLog } from '$lib/api';
	import { Button } from '$lib/components/ui/button';
	import { toast } from 'svelte-sonner';
	import { RefreshCw, ArrowLeft } from '@lucide/svelte';
	import AuditTable from '$lib/components/AuditTable.svelte';

	let { data } = $props();

	let auditLogs: AuditLog[] = $state(data.auditLogs || []);
	let loading = $state(false);
	let error: string | null = $state(null);

	async function fetchAuditLogs() {
		if (!browser) return;
		loading = true;
		error = null;
		try {
			const fetchedLogs = await listAuditLogs(100);
			auditLogs = fetchedLogs;
		} catch (err: any) {
			const msg = err.message || 'Failed to fetch audit logs';
			error = msg;
			toast.error(msg);
		} finally {
			loading = false;
		}
	}
</script>

<svelte:head>
	<title>Audit Logs - Object Registry</title>
</svelte:head>

<div class="container mx-auto max-w-7xl space-y-8 py-10">
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
				fetchAuditLogs();
			}}
			disabled={loading}
		>
			<RefreshCw class="h-4 w-4 {loading ? 'animate-spin' : ''}" />
		</Button>
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
