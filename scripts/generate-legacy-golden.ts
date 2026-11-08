import { DefaultScheduler, Rating, FSRSVersion } from 'ts-fsrs'
import { writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

type Step = { at: string; rating: number; card: any }

const here = dirname(fileURLToPath(import.meta.url))

async function scenario(name: string, description: string, ratings: number[], start = '2026-01-01T00:00:00.000Z') {
  const s = await DefaultScheduler({
    version: 'FSRS-7', desiredRetention: 0.9, enableShortTerm: true,
    learningSteps: ['1m', '10m'], relearningSteps: ['10m'],
    enableFuzz: false, allowModelOverride: false,
  })
  let now = new Date(start)
  let card: any = s.newCard({ now })
  const steps: Step[] = [{ at: now.toISOString(), rating: 0, card: snapshot(card) }]
  for (const rating of ratings) {
    const result = s.review({ card, grade: rating as Rating, now })
    card = result.card
    steps.push({ at: now.toISOString(), rating, card: snapshot(card) })
    now = new Date(card.dueAt)
  }
  return { name, description, steps }
}

function snapshot(card: any) {
  return {
    stability: card.stability,
    stability_fast: card.stabilityFast,
    difficulty: card.difficulty,
    state: card.state,
    due_at: card.dueAt.toISOString(),
    last_review_at: card.lastReviewAt ? card.lastReviewAt.toISOString() : null,
    scheduled_days: card.scheduledDays,
    learning_step: card.learningStep,
    reps: card.reps,
    lapses: card.lapses,
  }
}

async function main() {
  const scenarios = []
  scenarios.push(await scenario('new_good_graduation', 'New -> Good(10m) -> Good graduates to review -> Good', [3, 3, 3]))
  for (const r of [1, 2, 3, 4]) {
    scenarios.push(await scenario(`new_${r}`, `New card rated ${r}, then Good twice`, [r, 3, 3]))
  }
  scenarios.push(await scenario('learning_again_then_good', 'Good, Again at 10m, Good, Good', [3, 1, 3, 3]))
  scenarios.push(await scenario('learning_hard_then_good', 'Good, Hard, Good', [3, 2, 3]))
  scenarios.push(await scenario('learning_repeated_again', 'Good, Again, Again, Good', [3, 1, 1, 3]))
  scenarios.push(await scenario('review_lapse', 'Graduate, Review Again (lapse), Good in relearning', [3, 3, 1, 3]))
  scenarios.push(await scenario('review_lapse_twice', 'Graduate, Review Again, Again, Good', [3, 3, 1, 1, 3]))
  scenarios.push(await scenario('review_hard', 'Graduate, Review Hard', [3, 3, 2]))
  scenarios.push(await scenario('review_easy', 'Graduate, Review Easy', [3, 3, 4]))
  scenarios.push(await scenario('long_sequence', 'Eight Good ratings from New', [3, 3, 3, 3, 3, 3, 3, 3]))
  const out = {
    generated_by: 'ts-fsrs@6.0.0 DefaultScheduler (FSRS-7)',
    ts_fsrs_version: FSRSVersion,
    settings: {
      version: 'FSRS-7', desiredRetention: 0.9, enableShortTerm: true,
      learningSteps: ['1m', '10m'], relearningSteps: ['10m'],
      enableFuzz: false, allowModelOverride: false,
      graduatingIntervalDays: 1, maximumIntervalDays: 36500,
    },
    scenarios,
  }
  writeFileSync(join(here, '..', 'fixtures', 'legacy-fsrs7-golden.json'), JSON.stringify(out, null, 2) + '\n')
}

main()
