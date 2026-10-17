import { Component, Input } from '@angular/core';

@Component({
  selector: 'app-logo',
  template: `
    <svg class="logo" viewBox="0 0 128 128" role="img" aria-label="Moon Rabbit">
      <path
        d="M45 52C43 45 37 28 40 22C42 17 48 17 52 22L57 29C59 32 56 36 53 34L48 29C49 36 52 44 56 50ZM67 49C66 40 69 19 77 18C87 18 81 41 77 53Z"
        fill="var(--sage)"
        stroke="currentColor"
        stroke-width="3"
        stroke-linejoin="round"
      />
      <path
        d="M73 43L77 26"
        fill="none"
        stroke="var(--bg)"
        stroke-width="4"
        stroke-linecap="round"
      />
      <path d="M27 78A37 37 0 0 1 101 78Z" fill="var(--sage)" />
      <path d="M27 78A37 37 0 0 1 101 78" fill="none" stroke="currentColor" stroke-width="3" />
      <path
        d="M49 65Q53 61 57 65M71 65Q75 61 79 65"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
      />
      <path
        d="M60 69Q64 74 68 69"
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
      />
      <path
        d="M43 70H48M80 70H85"
        fill="none"
        stroke="var(--bg)"
        stroke-width="4"
        stroke-linecap="round"
        opacity=".75"
      />
      <path
        d="M17 78H111M29 92H99M44 106H84"
        fill="none"
        stroke="currentColor"
        stroke-width="4.5"
        stroke-linecap="round"
      />
    </svg>
  `,
})
export class Logo {
  @Input() label = 'Moon Rabbit';
}
