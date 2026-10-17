import { HttpClient, httpResource } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import {
  AddWordsResult,
  Dashboard,
  DraftLine,
  GradeRequest,
  GradeResult,
  ListPage,
  ListSummary,
  Session,
  SessionRequest,
  WordDetail,
  WordInput,
  ScreenshotResult,
  WordSearchResult,
} from '@moonrabbit/contract';

@Injectable({ providedIn: 'root' })
export class Api {
  private http = inject(HttpClient);

  dashboard() {
    return httpResource<Dashboard>(() => '/api/dashboard');
  }

  lists() {
    return httpResource<ListSummary[]>(() => '/api/lists');
  }

  list(request: () => { id: string; cursor: number } | undefined) {
    return httpResource<ListPage>(() => {
      const page = request();
      return page
        ? { url: `/api/lists/${encodeURIComponent(page.id)}`, params: { cursor: page.cursor } }
        : undefined;
    });
  }

  word(id: () => string | undefined) {
    return httpResource<WordDetail>(() => {
      const value = id();
      return value ? `/api/words/${encodeURIComponent(value)}` : undefined;
    });
  }

  createList(title: string) {
    return this.http.post<{ id: string; title: string }>('/api/lists', { title });
  }

  setQueue(ids: string[], version: number) {
    return this.http.put('/api/queue', { ids, version });
  }

  search(query: () => string | undefined) {
    return httpResource<WordSearchResult[]>(() => {
      const q = query();
      return q ? { url: '/api/words', params: { q } } : undefined;
    });
  }

  screenshot(image: string) {
    return this.http.post<ScreenshotResult>('/api/words/screenshot', { image });
  }

  generateExample(wordId: string) {
    return this.http.post<WordDetail>(`/api/words/${encodeURIComponent(wordId)}/examples`, {});
  }

  drafts(lines: string[]) {
    return this.http.post<DraftLine[]>('/api/words/drafts', { lines });
  }

  addWords(listId: string, words: WordInput[]) {
    return this.http.post<AddWordsResult>(`/api/lists/${listId}/words`, { words });
  }

  removeWord(listId: string, wordId: string) {
    return this.http.delete(`/api/lists/${listId}/words/${wordId}`);
  }

  session(request: () => SessionRequest | undefined) {
    return httpResource<Session>(() => {
      const body = request();
      return body ? { url: '/api/session', method: 'POST', body } : undefined;
    });
  }

  grade(body: GradeRequest) {
    return this.http.post<GradeResult>('/api/grades', body);
  }

  speech(wordId: string, sentenceId?: string) {
    return this.http.post(
      '/api/speech',
      { wordId, ...(sentenceId ? { sentenceId } : {}) },
      { responseType: 'arraybuffer' },
    );
  }
}
