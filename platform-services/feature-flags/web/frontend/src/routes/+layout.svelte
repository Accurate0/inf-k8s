<script lang="ts">
  import "../app.css";
  import { page } from "$app/state";

  let { children } = $props();

  const tabs = [
    { href: "/", label: "Flags" },
    { href: "/segments", label: "Segments" },
    { href: "/debug", label: "Debug" },
  ];

  function active(href: string): boolean {
    if (href === "/") return page.url.pathname === "/" || page.url.pathname.startsWith("/flags");
    return page.url.pathname.startsWith(href);
  }
</script>

<div class="min-h-screen bg-muted/30">
  <header class="bg-foreground text-background">
    <div class="mx-auto flex max-w-5xl items-center gap-8 px-6">
      <span class="py-4 text-base font-semibold">Feature Flags</span>
      <nav class="flex gap-1">
        {#each tabs as tab (tab.href)}
          <a
            href={tab.href}
            class="border-b-2 px-3 py-4 text-sm transition-colors {active(tab.href)
              ? 'border-primary-foreground text-background'
              : 'border-transparent text-background/60 hover:text-background'}"
          >
            {tab.label}
          </a>
        {/each}
      </nav>
    </div>
  </header>

  <main class="mx-auto max-w-5xl px-6 py-8">
    {@render children()}
  </main>
</div>
