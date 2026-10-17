import { Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Api } from '../core/api';
import { Shell } from '../shell';

@Component({
  selector: 'app-home',
  imports: [Shell, RouterLink],
  templateUrl: './home.html',
})
export class HomePage {
  readonly dashboard = inject(Api).dashboard();
  readonly loading = this.dashboard.isLoading;
  readonly data = computed(() => (this.dashboard.hasValue() ? this.dashboard.value() : null));
  readonly error = computed(() =>
    this.dashboard.error()
      ? 'The dashboard could not be loaded. Check the local API, then retry.'
      : '',
  );

  load() {
    this.dashboard.reload();
  }

  percent(value: number | null) {
    return value == null ? '—' : `${Math.round(value * 100)}%`;
  }
}
