import { SHOT_ASPECT, visibleShots } from '../components/Screenshots';

const SHOT = 'https://bridgething.com/screenshots/device-calendar.png';
const STEAL = "javascript:fetch('https://evil.example/')";

describe('the mobile screenshot strip', () => {
  test('keeps the order the catalog published', () => {
    const second = SHOT.replace('calendar', 'weather');
    expect(visibleShots([SHOT, second])).toEqual([SHOT, second]);
  });

  test('an app with no screenshots has nothing to show', () => {
    expect(visibleShots(undefined)).toEqual([]);
    expect(visibleShots([])).toEqual([]);
  });

  test('a javascript url from a hostile catalog never reaches an image source', () => {
    expect(visibleShots([STEAL, SHOT])).toEqual([SHOT]);
  });

  test('a capture that failed to load drops out instead of leaving a hole', () => {
    expect(visibleShots([SHOT], [SHOT])).toEqual([]);
  });

  test('the cards match the device screen so a capture is never letterboxed', () => {
    expect(SHOT_ASPECT).toBeCloseTo(800 / 480, 1);
  });
});
