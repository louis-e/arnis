$execute if data storage arnis:cars spawn{kind:"van"} run data modify storage arnis:cars spawn.colour set from storage arnis:cars colours.van[$(k)]
$execute unless data storage arnis:cars spawn{kind:"van"} run data modify storage arnis:cars spawn.colour set from storage arnis:cars colours.car[$(k)]
