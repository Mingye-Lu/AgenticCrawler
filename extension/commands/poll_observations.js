'use strict';

async function handlePollObservations(tabId) {
  return { events: drainObservationBuffer(tabId) };
}
