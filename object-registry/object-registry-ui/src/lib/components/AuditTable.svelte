<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Loader2, History } from '@lucide/svelte';
	import type { AuditLog } from '$lib/api';

	let {
		auditLogs = [],
		loading = false,
		error = null
	} = $props();

	function formatDetails(details: Record<string, string>) {
		return Object.entries(details)
			.map(([k, v]) => `${k}=${v}`)
			.join(', ');
	}
</script>

<div class="space-y-4">
	<Card.Root>
		<Card.Content class="p-0 overflow-x-auto">
			<Table.Root>
				<Table.Header>
					<Table.Row>
						<Table.Head class="pl-6 min-w-[160px]">Timestamp</Table.Head>
						<Table.Head>Action</Table.Head>
						<Table.Head>Subject</Table.Head>
						<Table.Head class="hidden sm:table-cell">Namespace</Table.Head>
						<Table.Head class="hidden xl:table-cell">Object Key</Table.Head>
						<Table.Head class="pr-6 min-w-[200px]">Details</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#if loading && auditLogs.length === 0}
						<Table.Row>
							<Table.Cell colspan={6} class="h-32 text-center">
								<div class="flex flex-col items-center justify-center space-y-2">
									<Loader2 class="h-8 w-8 animate-spin text-muted-foreground" />
									<p class="animate-pulse text-muted-foreground">Loading audit logs...</p>
								</div>
							</Table.Cell>
						</Table.Row>
					{:else if auditLogs.length === 0 && !error}
						<Table.Row>
							<Table.Cell colspan={6} class="h-32 text-center">
								<div class="flex flex-col items-center justify-center space-y-2">
									<History class="h-12 w-12 text-muted-foreground/50" />
									<p class="text-muted-foreground">No audit logs found.</p>
								</div>
							</Table.Cell>
						</Table.Row>
					{:else}
						{#each auditLogs as log (log.id)}
							<Table.Row>
								<Table.Cell class="pl-6 font-medium whitespace-nowrap">
									{new Date(log.timestamp).toLocaleString()}
								</Table.Cell>
								<Table.Cell>
									<span class="inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold bg-primary/10 text-primary whitespace-nowrap">
										{log.action}
									</span>
								</Table.Cell>
								<Table.Cell class="font-mono text-xs max-w-[150px] truncate" title={log.subject}>{log.subject}</Table.Cell>
								<Table.Cell class="hidden text-muted-foreground sm:table-cell"
									>{log.namespace || '-'}</Table.Cell
								>
								<Table.Cell class="hidden xl:table-cell font-mono text-xs">{log.object_key || '-'}</Table.Cell>
								<Table.Cell class="pr-6 text-xs text-muted-foreground max-w-xs truncate" title={formatDetails(log.details)}>
									{formatDetails(log.details)}
								</Table.Cell>
							</Table.Row>
						{/each}
					{/if}
				</Table.Body>
			</Table.Root>
		</Card.Content>
	</Card.Root>
</div>
