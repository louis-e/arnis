scoreboard players set #tt arnis.train 0
# One clock for every timetable, so a train is where it would be had it run all along.
scoreboard players add #now arnis.train 1
function arnis:trains/all
scoreboard players add #sweep arnis.train 1
execute if score #sweep arnis.train matches 10.. run function arnis:trains/sweep
