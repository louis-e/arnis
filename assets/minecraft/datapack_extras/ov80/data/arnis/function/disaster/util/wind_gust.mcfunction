# Only out in the open: a roof overhead is enough shelter.
function arnis:disaster/util/outdoors
execute if score #out arnis.ds matches 0 run return 0
function arnis:disaster/util/wind_push with storage arnis:ds wind
