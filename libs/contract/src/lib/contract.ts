export const LIST_PAGE_SIZE = 100;
export const WORD_BATCH_SIZE = 40;
export const SESSION_AMOUNTS = [2, 3, 6, 10, 20] as const;

export type Direction = 'recognition' | 'production';
export type Rating = 'again' | 'good';
export type SessionUnit = 'minutes' | 'words';
export type CardPhase = 'new' | 'learning' | 'review' | 'relearning';

export interface ExampleSentenceInput {
  simplified: string;
  pinyin: string;
  pronouncedPinyin: string;
  translation: string;
}

export interface ExampleSentence extends ExampleSentenceInput {
  id: string;
}

export interface WordListReference {
  id: string;
  title: string;
}

export interface Word {
  id: string;
  simplified: string;
  pinyin: string;
  pronouncedPinyin?: string;
  translation: string;
}

export interface WordSearchResult extends Word {
  lists: WordListReference[];
}

export interface ListSummary {
  id: string;
  title: string;
  owner: string;
  words: number;
  wordsLeft: number;
  charactersStudied: number;
  characterCount: number;
}

export interface ListPage {
  id: string;
  title: string;
  owner: string;
  words: ListWord[];
  nextCursor: number | null;
  totalWords: number;
}

export type CardStat = {
  direction: Direction;
  reps?: number;
  lapses?: number;
  due?: number;
  lastReview?: number;
  stability?: number;
} & ({ studied: false; state?: never } | { studied: true; state: CardPhase });

export interface WordDetail extends Word {
  cards: CardStat[];
  examples: ExampleSentence[];
}

export interface ListWord extends Word {
  studied: boolean;
}

export interface DashboardDay {
  date: string;
  studied: boolean;
}

export interface Dashboard {
  due: number;
  recallable: number;
  forgotten: number;
  charactersStudied: number;
  retention: number | null;
  days: DashboardDay[];
  queue: ListSummary[];
  queueVersion: number;
}

export interface DraftReading {
  pinyin: string;
  pronouncedPinyin?: string;
  translation: string;
  examples?: ExampleSentenceInput[];
}

export interface DraftLine {
  simplified: string;
  readings: DraftReading[];
}
export interface ScreenshotWordDraft extends DraftLine {
  matches: WordSearchResult[];
}

export interface ScreenshotResult {
  words: ScreenshotWordDraft[];
  warnings: string[];
}

export interface Face {
  wordId: string;
  direction: Direction;
  simplified: string;
  pinyin: string;
  pronouncedPinyin?: string;
  translation: string;
}

export interface SessionRequest {
  unit: SessionUnit;
  amount: number;
  direction: Direction;
}

export interface Session {
  cards: Face[];
}

export interface GradeRequest {
  eventId: string;
  wordId: string;
  direction: Direction;
  rating: Rating;
  reviewedAt: number;
}

export interface GradeResult {
  status: 'applied' | 'duplicate' | 'stale';
}

export interface AddWordsResult {
  added: number;
  rejected: { index: number; error: string }[];
}

export interface WordInput {
  simplified: string;
  pinyin: string;
  translation: string;
  pronouncedPinyin?: string;
  examples?: ExampleSentenceInput[];
}
