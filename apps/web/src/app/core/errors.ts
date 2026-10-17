import { HttpErrorResponse } from '@angular/common/http';

export function apiErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof HttpErrorResponse) {
    let body: unknown = error.error;
    // Speech requests use arraybuffer responses, including JSON error bodies.
    if (body instanceof ArrayBuffer) {
      try {
        body = JSON.parse(new TextDecoder().decode(body));
      } catch {
        return fallback;
      }
    }
    if (
      body &&
      typeof body === 'object' &&
      'message' in body &&
      typeof body.message === 'string' &&
      body.message.trim()
    )
      return body.message;
    return fallback;
  }
  return error instanceof Error && error.message ? error.message : fallback;
}
