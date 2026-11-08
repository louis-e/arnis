const DEFAULT_LOCALE_PATH = `./locales/en.json`;

// Browser tags of languages Arnis files under another code.
const LANGUAGE_ALIASES = { uk: 'ua' };

// Chinese as written in Taiwan, Hong Kong and Macau, in the traditional script zh-CN is not.
const TRADITIONAL_CHINESE = /^zh-(hant|tw|hk|mo)\b/i;

/**
 * Picks the offered language closest to a browser tag: the tag itself, its
 * base language, or the same language in another region, as fr-FR for fr-CA.
 * @param {string} tag - A browser language tag such as navigator.language
 * @param {string[]} available - The offered language codes
 * @returns {string} One of the offered codes, or 'en'
 */
export function matchLanguage(tag, available) {
    const base = (tag || 'en').split('-')[0].toLowerCase();
    const code = [tag, LANGUAGE_ALIASES[base] || base].find(c => available.includes(c));
    if (code) return code;
    if (TRADITIONAL_CHINESE.test(tag || '')) return 'en';
    return available.find(c => c.split('-')[0] === base) || 'en';
}

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
