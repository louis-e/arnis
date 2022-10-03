# Check if black formatter will work without any rewrites, and produces an exit code
style-check:
	black src/ --check

# This will reformat all python files unders bookdifferent/ into the python black standard
style:
	black src/

# Checks that the python source files are compliant regarding errors and style conventions
lint:
	flake8 src/