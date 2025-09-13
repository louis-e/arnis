const DEFAULT_LOCALE_PATH = `./locales/en.json`;

/**
 * Checks if a JSON response is invalid or falls back to HTML
 * @param {Response} response - The fetch response object
 * @returns {boolean} True if the response is invalid JSON
 */
export function invalidJSON(response) {
    return !response.ok || response.headers.get("Content-Type") === "text/html";
}

/**
 * Fetches a specific language file
 * @param {string} languageCode - The language code to fetch
 * @returns {Promise<Object>} The localization JSON object
 */
export async function fetchLanguage(languageCode) {

    let response = await fetch(`./locales/${languageCode}.json`);

    // Try with only first part of language code if not found
    if (invalidJSON(response)) {
        response = await fetch(`./locales/${languageCode.split('-')[0]}.json`);

    // Fallback to default English localization
        if (invalidJSON(response)) {
            response = await fetch(DEFAULT_LOCALE_PATH);
        }
    }

    const localization = await response.json();
    return localization;
}
