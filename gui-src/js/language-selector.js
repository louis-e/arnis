// Function to handle language selection
function handleLanguageSelection(selectElement) {
    const selectedValue = selectElement.value;
    // No flag-related styling is needed anymore
    // This function remains as a placeholder in case we need to add functionality later
}

// Wait for DOM to be fully loaded
document.addEventListener('DOMContentLoaded', function () {
    // Get language selector
    const languageSelect = document.getElementById('language-select');

    if (languageSelect) {
        // Set initial value based on saved preference or browser language
        const savedLanguage = localStorage.getItem('arnis-language');
        const currentLang = savedLanguage || navigator.language;
        const availableOptions = Array.from(languageSelect.options).map(opt => opt.value);

        // Try to match the exact language code first
        if (availableOptions.includes(currentLang)) {
            languageSelect.value = currentLang;
        }
        // Try to match just the base language code
        else if (availableOptions.includes(currentLang.split('-')[0])) {
            languageSelect.value = currentLang.split('-')[0];
        }
        // Initialize language selection
        handleLanguageSelection(languageSelect);

        // Handle language change
        languageSelect.addEventListener('change', function () {
            const selectedLanguage = languageSelect.value;

            // Store selection in localStorage
            localStorage.setItem('arnis-language', selectedLanguage);
            // Reload localization with the new language
            if (window.fetchLanguage) {
                window.fetchLanguage(selectedLanguage).then(localization => {
                    if (window.applyLocalization) {
                        window.applyLocalization(localization);

                        // Re-initialize the footer to ensure year and version are properly displayed
                        if (window.initFooter) {
                            window.initFooter();
                        }
                    }
                });
            } else {
                // If the fetchLanguage function isn't exposed to window, just reload the page
                window.location.reload();
            }
        });
    }
});
