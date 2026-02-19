import type { PageServerLoad } from './$types';
import type { AuditLog } from '$lib/api';

export const load: PageServerLoad = async ({ fetch, url }) => {
	const response = await fetch(`/api/object-registry/audit${url.search || '?limit=100'}`);
	let auditLogs: AuditLog[] = [];

	if (response.ok) {
		try {
			auditLogs = await response.json();
		} catch (err) {
			console.error('Failed to parse audit logs:', err);
		}
	} else {
		console.error('Failed to fetch audit logs:', response.statusText);
	}

	// Also fetch unfiltered logs to get a better set of suggestions if filters are active
	let suggestionLogs: AuditLog[] = auditLogs;
	if (url.search) {
		const suggestionResponse = await fetch('/api/object-registry/audit?limit=200');
		if (suggestionResponse.ok) {
			suggestionLogs = await suggestionResponse.json();
		}
	}

	const suggestions = {
		actions: [...new Set(suggestionLogs.map((log) => log.action))].sort(),
		subjects: [...new Set(suggestionLogs.map((log) => log.subject))].sort(),
		namespaces: [...new Set(suggestionLogs.filter(log => log.namespace).map((log) => log.namespace as string))].sort()
	};

	return {
		auditLogs,
		suggestions
	};
};
