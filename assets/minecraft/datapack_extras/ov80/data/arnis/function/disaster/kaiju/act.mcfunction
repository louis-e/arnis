tag @s add arnis.ds.here
execute if score #kj arnis.ds matches 1 run function arnis:disaster/kaiju/rise
execute if score #kj arnis.ds matches 2 run function arnis:disaster/kaiju/walk
execute if score #kj arnis.ds matches 3 run function arnis:disaster/kaiju/sink
tag @s remove arnis.ds.here
