$execute store result score #kind arnis.ds run data get storage arnis:ds bag[$(i)]
$data remove storage arnis:ds bag[$(i)]
execute store result storage arnis:ds pick.k int 1 run scoreboard players get #kind arnis.ds
function arnis:disaster/bag/text with storage arnis:ds pick
