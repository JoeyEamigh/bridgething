import type { ConditionId } from './status';
import { getVoiceIntroOutcome } from './storage';

export type SetupStep = 'signIn' | 'pair' | 'voice' | 'permissions';

export function setupSteps(paired: boolean): SetupStep[] {
  const steps: SetupStep[] = ['signIn', 'pair'];
  if (paired && getVoiceIntroOutcome() === null) steps.push('voice');
  steps.push('permissions');
  return steps;
}

const STEP_CONDITION: Partial<Record<SetupStep, ConditionId>> = {
  signIn: 'notSignedIn',
  pair: 'noDevice',
};

export function stepForCondition(
  steps: SetupStep[],
  condition: ConditionId,
): number | null {
  const index = steps.findIndex(step => STEP_CONDITION[step] === condition);
  return index === -1 ? null : index;
}

export function pendingSummary(count: number): string {
  return count === 0 ? 'all set' : `${count} left`;
}
