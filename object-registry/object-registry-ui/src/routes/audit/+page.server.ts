import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ fetch }) => {
	const response = await fetch('/api/object-registry/audit?limit=100');
	let auditLogs = [];

	if (response.ok) {
		try {
			auditLogs = await response.json();
		} catch (err) {
			console.error('Failed to parse audit logs:', err);
		}
	} else {
		console.error('Failed to fetch audit logs:', response.statusText);
	}

	return {
		auditLogs
	};
};
